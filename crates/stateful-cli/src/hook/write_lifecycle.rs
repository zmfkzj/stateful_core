use std::{
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use stateful_core::{ActorType, ContentFingerprint, Decision, SourceKind, WriteIntentOutcome};

use crate::{GlobalPaths, RepoIdentity, ServerRuntime};

#[derive(Debug, Deserialize, Serialize)]
struct PendingIntent {
    intent_id: String,
    targets: Vec<String>,
    #[serde(default)]
    claim_ids: Vec<String>,
    #[serde(default)]
    completion_request: Option<String>,
    #[serde(default)]
    completed: bool,
    #[serde(default)]
    release_requests: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct IntentResponse {
    intent_id: String,
    #[serde(default)]
    decision: Option<Decision>,
}

pub(crate) struct Authorization {
    pub(crate) decision: Option<Decision>,
}

#[derive(Debug)]
pub(crate) struct AuthorizationDenied {
    pub(crate) decision: Decision,
}

impl fmt::Display for AuthorizationDenied {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}: {}",
            self.decision.reason_code, self.decision.message
        )?;
        if let Some(action) = &self.decision.required_next_action {
            write!(formatter, " {action}")?;
        }
        Ok(())
    }
}

impl std::error::Error for AuthorizationDenied {}

pub(crate) struct LifecycleSource {
    pub(crate) source_kind: SourceKind,
    pub(crate) start_event: &'static str,
    pub(crate) start_ref: &'static str,
    pub(crate) complete_event: &'static str,
    pub(crate) complete_ref: &'static str,
}

#[expect(
    clippy::too_many_arguments,
    reason = "write authorization carries protocol and lifecycle identity"
)]
pub(crate) fn authorize(
    paths: &GlobalPaths,
    runtime: &ServerRuntime,
    agent_id: &str,
    workspace_id: &str,
    identity: Option<&RepoIdentity>,
    actor_id: Option<&str>,
    parent_agent_id: Option<&str>,
    operation_id: &str,
    action: &str,
    targets: Vec<(String, ContentFingerprint)>,
    reservation_id: Option<&str>,
    claim_ids: Vec<String>,
    source: &LifecycleSource,
) -> anyhow::Result<Authorization> {
    let mut request = crate::v2_request_envelope(
        uuid::Uuid::new_v4(),
        agent_id.to_string(),
        workspace_id.to_string(),
        identity.cloned(),
        ActorType::Agent,
        source.source_kind.clone(),
        source.start_event,
        source.start_ref,
        Some(action.to_string()),
        json!({
            "operation_id": operation_id,
            "action": action,
            "targets": targets
                .iter()
                .map(|(path, before)| json!({ "path": path, "before": before }))
                .collect::<Vec<_>>(),
            "reservation_id": reservation_id,
        }),
    )?;
    if let Some(actor_id) = actor_id {
        request.agent.actor_id = actor_id.to_string();
    }
    request.agent.parent_agent_id = parent_agent_id.map(str::to_string);
    let response = crate::post_v2_raw(runtime, "/v2/authorize", &request)?;
    if !(200..300).contains(&response.status_code) {
        if let Ok(decision) = serde_json::from_str::<Decision>(&response.body) {
            return Err(AuthorizationDenied { decision }.into());
        }
        anyhow::bail!(
            "stateful.v2 authorization failed with HTTP {}: {}",
            response.status_code,
            response.body
        );
    }
    let IntentResponse {
        intent_id,
        decision,
    } = serde_json::from_str(&response.body)?;
    save_pending(
        paths,
        agent_id,
        operation_id,
        &PendingIntent {
            intent_id,
            targets: targets.into_iter().map(|(path, _)| path).collect(),
            claim_ids,
            completion_request: None,
            release_requests: Vec::new(),
            completed: false,
        },
    )?;
    Ok(Authorization { decision })
}

#[expect(
    clippy::too_many_arguments,
    reason = "write completion carries protocol and lifecycle identity"
)]
pub(crate) fn complete(
    paths: &GlobalPaths,
    runtime: &ServerRuntime,
    agent_id: &str,
    workspace_id: &str,
    identity: Option<&RepoIdentity>,
    actor_id: Option<&str>,
    parent_agent_id: Option<&str>,
    repo_root: &Path,
    operation_id: &str,
    failed: bool,
    source: &LifecycleSource,
) -> anyhow::Result<bool> {
    let Some(mut intent) = load_pending(paths, agent_id, operation_id)? else {
        return Ok(false);
    };
    if !intent.completed {
        if let Some(request) = intent.completion_request.as_deref() {
            if crate::replay_v2_request(runtime, "/v2/write/complete", request).is_err() {
                return Ok(true);
            }
        } else {
            let post_fingerprints = if failed {
                Vec::new()
            } else {
                intent
                    .targets
                    .iter()
                    .map(|path| {
                        Ok((
                            path.clone(),
                            stateful_core::fingerprint_path(&repo_root.join(path))?,
                        ))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?
            };
            let outcome = if failed {
                WriteIntentOutcome::Failed
            } else {
                WriteIntentOutcome::Committed
            };
            let mut request = crate::v2_request_envelope(
                uuid::Uuid::new_v4(),
                agent_id.to_string(),
                workspace_id.to_string(),
                identity.cloned(),
                ActorType::Agent,
                source.source_kind.clone(),
                source.complete_event,
                source.complete_ref,
                None,
                json!({
                    "intent_id": intent.intent_id,
                    "outcome": outcome,
                    "post_fingerprints": post_fingerprints,
                    "failure_code": failed.then_some("tool_failed"),
                }),
            )?;
            if let Some(actor_id) = actor_id {
                request.agent.actor_id = actor_id.to_string();
            }
            request.agent.parent_agent_id = parent_agent_id.map(str::to_string);
            intent.completion_request = Some(serde_json::to_string(&request)?);
            save_pending(paths, agent_id, operation_id, &intent)?;
            if crate::replay_v2_request(
                runtime,
                "/v2/write/complete",
                intent
                    .completion_request
                    .as_deref()
                    .expect("completion request was just saved"),
            )
            .is_err()
            {
                return Ok(true);
            }
        }
        intent.completion_request = None;
        intent.completed = true;
    }
    if intent.release_requests.is_empty() {
        intent.release_requests = intent
            .claim_ids
            .iter()
            .map(|claim_id| {
                let mut request = crate::v2_request_envelope(
                    uuid::Uuid::new_v4(),
                    agent_id.to_string(),
                    workspace_id.to_string(),
                    identity.cloned(),
                    ActorType::Agent,
                    source.source_kind.clone(),
                    "claim_release",
                    source.complete_ref,
                    None,
                    json!({ "claim_id": claim_id }),
                )?;
                if let Some(actor_id) = actor_id {
                    request.agent.actor_id = actor_id.to_string();
                }
                request.agent.parent_agent_id = parent_agent_id.map(str::to_string);
                serde_json::to_string(&request).map_err(anyhow::Error::from)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
    }
    save_pending(paths, agent_id, operation_id, &intent)?;

    while let Some(request) = intent.release_requests.first() {
        if crate::replay_v2_request(runtime, "/v2/claim/release", request).is_err() {
            return Ok(true);
        }
        intent.release_requests.remove(0);
        save_pending(paths, agent_id, operation_id, &intent)?;
    }
    fs::remove_file(pending_path(paths, agent_id, operation_id))?;
    Ok(true)
}

fn save_pending(
    paths: &GlobalPaths,
    agent_id: &str,
    operation_id: &str,
    intent: &PendingIntent,
) -> anyhow::Result<()> {
    let path = pending_path(paths, agent_id, operation_id);
    let parent = path.parent().expect("pending path has a parent");
    fs::create_dir_all(parent)?;
    fs::write(path, serde_json::to_vec(intent)?)?;
    Ok(())
}

fn load_pending(
    paths: &GlobalPaths,
    agent_id: &str,
    operation_id: &str,
) -> anyhow::Result<Option<PendingIntent>> {
    let path = pending_path(paths, agent_id, operation_id);
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn pending_path(paths: &GlobalPaths, agent_id: &str, operation_id: &str) -> PathBuf {
    paths
        .runtime_dir
        .join("write-intents")
        .join(hex(agent_id))
        .join(format!("{}.json", hex(operation_id)))
}

fn hex(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
    };

    use super::*;

    const SOURCE: LifecycleSource = LifecycleSource {
        source_kind: SourceKind::Hook,
        start_event: "test_write_start",
        start_ref: "test",
        complete_event: "test_write_complete",
        complete_ref: "test",
    };
    const AUTHORIZED: &str =
        r#"{"intent_id":"intent-1","decision":{"decision":"allow","reason_code":"allowed","message":"ok"}}"#;
    const IDENTITY: &str =
        r#"{"protocol_version":"stateful.v2","journal_schema_version":2,"coordination_mode":"awareness","pid":1,"workspace_id":"workspace-1","workspace_version":1,"capabilities":["presence"]}"#;

    #[test]
    fn retries_failed_write_completion_with_the_original_request_and_operation_ids() {
        let temp = tempfile::tempdir().expect("temp dir should create");
        let paths = GlobalPaths::new(temp.path().join("home"));
        let repo_root = temp.path().join("repo");
        fs::create_dir_all(&repo_root).expect("repo should create");
        fs::write(repo_root.join("target.txt"), "after").expect("target should write");
        let (runtime, requests) = fake_server([Some(AUTHORIZED), None, Some(r#"{"status":"ok"}"#)]);

        authorize(
            &paths,
            &runtime,
            "agent-1",
            "workspace-1",
            None,
            None,
            None,
            "operation-1",
            "write_file",
            vec![(
                "target.txt".to_string(),
                stateful_core::ContentFingerprint::missing(),
            )],
            None,
            Vec::new(),
            &SOURCE,
        )
        .expect("authorization should persist its intent");
        let _authorize = requests.recv().expect("authorization should arrive");

        assert!(
            complete(
                &paths,
                &runtime,
                "agent-1",
                "workspace-1",
                None,
                None,
                None,
                &repo_root,
                "operation-1",
                false,
                &SOURCE,
            )
            .expect("failed completion should retain replay state"),
            "pending operation should be handled"
        );
        let first_completion = requests.recv().expect("initial completion should arrive");
        let initial = request_body(&first_completion);
        assert_eq!(initial["payload"]["intent_id"], "intent-1");
        assert!(pending_path(&paths, "agent-1", "operation-1").exists());

        assert!(
            complete(
                &paths,
                &runtime,
                "agent-1",
                "workspace-1",
                None,
                None,
                None,
                &repo_root,
                "operation-1",
                false,
                &SOURCE,
            )
            .expect("completion recovery should replay"),
        );
        let replay = request_body(&requests.recv().expect("replayed completion should arrive"));
        assert_eq!(replay, initial, "replay must retain the original request UUID");
        assert!(!pending_path(&paths, "agent-1", "operation-1").exists());
    }

    #[test]
    fn retains_completed_intent_until_claim_release_replays() {
        let temp = tempfile::tempdir().expect("temp dir should create");
        let paths = GlobalPaths::new(temp.path().join("home"));
        let repo_root = temp.path().join("repo");
        fs::create_dir_all(&repo_root).expect("repo should create");
        fs::write(repo_root.join("target.txt"), "after").expect("target should write");
        let (runtime, requests) =
            fake_server([Some(AUTHORIZED), Some(r#"{"status":"ok"}"#), None, Some(r#"{"status":"ok"}"#)]);

        authorize(
            &paths,
            &runtime,
            "agent-1",
            "workspace-1",
            None,
            None,
            None,
            "operation-1",
            "write_file",
            vec![(
                "target.txt".to_string(),
                stateful_core::ContentFingerprint::missing(),
            )],
            None,
            vec!["claim-1".to_string()],
            &SOURCE,
        )
        .expect("authorization should persist its intent");
        let _authorize = requests.recv().expect("authorization should arrive");

        assert!(
            complete(
                &paths,
                &runtime,
                "agent-1",
                "workspace-1",
                None,
                None,
                None,
                &repo_root,
                "operation-1",
                false,
                &SOURCE,
            )
            .expect("release failure should retain replay state"),
        );
        let _completion = requests.recv().expect("completion should arrive");
        let first_release = request_body(&requests.recv().expect("release should arrive"));
        assert_eq!(first_release["payload"]["claim_id"], "claim-1");
        assert!(pending_path(&paths, "agent-1", "operation-1").exists());

        assert!(
            complete(
                &paths,
                &runtime,
                "agent-1",
                "workspace-1",
                None,
                None,
                None,
                &repo_root,
                "operation-1",
                false,
                &SOURCE,
            )
            .expect("release recovery should replay"),
        );
        let replay = request_body(&requests.recv().expect("release replay should arrive"));
        assert_eq!(replay, first_release, "claim release replay must retain its request UUID");
        assert!(!pending_path(&paths, "agent-1", "operation-1").exists());
    }

    fn fake_server<const N: usize>(
        responses: [Option<&'static str>; N],
    ) -> (ServerRuntime, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
        let addr = listener.local_addr().expect("listener address should load");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut responses = responses.into_iter();
            while let Some(response) = responses.next() {
                loop {
                    let (mut stream, _) = listener.accept().expect("request should connect");
                    let request = read_request(&mut stream);
                    if request.starts_with("GET /v2/runtime/identity?") {
                        write_response(&mut stream, IDENTITY);
                        continue;
                    }
                    tx.send(request).expect("request should send");
                    if let Some(body) = response {
                        write_response(&mut stream, body);
                    }
                    break;
                }
            }
        });
        (
            ServerRuntime::new(format!("http://{addr}"), "token", "workspace-1", 1),
            rx,
        )
    }

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut request = Vec::new();
        let mut byte = [0_u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            stream
                .read_exact(&mut byte)
                .expect("request header byte should read");
            request.push(byte[0]);
        }
        let headers = String::from_utf8(request.clone()).expect("headers should be UTF-8");
        let content_length = headers
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .map(|length| length.parse::<usize>().expect("content length should parse"))
            .unwrap_or(0);
        let mut body = vec![0_u8; content_length];
        stream
            .read_exact(&mut body)
            .expect("request body should read");
        request.extend(body);
        String::from_utf8(request).expect("request should be UTF-8")
    }

    fn request_body(request: &str) -> serde_json::Value {
        serde_json::from_str(
            request
                .split_once("\r\n\r\n")
                .expect("request should have a body")
                .1,
        )
        .expect("request body should be JSON")
    }

    fn write_response(stream: &mut std::net::TcpStream, body: &str) {
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("response should write");
    }
}
