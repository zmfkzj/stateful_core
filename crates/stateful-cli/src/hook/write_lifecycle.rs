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
    #[serde(default)]
    intent_id: Option<String>,
    #[serde(default)]
    authorization_request: String,
    targets: Vec<String>,
    #[serde(default)]
    repo_root: String,
    #[serde(default)]
    claim_ids: Vec<String>,
    #[serde(default)]
    completion_request: Option<String>,
    #[serde(default)]
    recovery_request: Option<String>,
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
    let authorization_request = serde_json::to_string(&request)?;
    let mut pending = load_pending(paths, agent_id, operation_id)?.unwrap_or(PendingIntent {
        intent_id: None,
        authorization_request: authorization_request.clone(),
        repo_root: request.workspace.root.clone(),
        targets: targets.iter().map(|(path, _)| path.clone()).collect(),
        claim_ids,
        completion_request: None,
        recovery_request: None,
        release_requests: Vec::new(),
        completed: false,
    });
    if pending.authorization_request.is_empty() {
        pending.authorization_request = authorization_request;
    }
    save_pending(paths, agent_id, operation_id, &pending)?;

    let response = post_frozen_authorization(runtime, &pending.authorization_request)?;
    if !(200..300).contains(&response.status_code) {
        if let Ok(decision) = serde_json::from_str::<Decision>(&response.body) {
            finish_denied_authorization(
                runtime,
                &pending_path(paths, agent_id, operation_id),
                &mut pending,
            )?;
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
    pending.intent_id = Some(intent_id);
    save_pending(paths, agent_id, operation_id, &pending)?;
    Ok(Authorization { decision })
}

fn post_frozen_authorization(
    runtime: &ServerRuntime,
    serialized_request: &str,
) -> anyhow::Result<crate::HttpResponse> {
    let request: stateful_core::RequestEnvelope<serde_json::Value> =
        serde_json::from_str(serialized_request)?;
    crate::post_v2_raw(runtime, "/v2/authorize", &request)
}

fn finish_denied_authorization(
    runtime: &ServerRuntime,
    path: &Path,
    intent: &mut PendingIntent,
) -> anyhow::Result<()> {
    intent.completed = true;
    freeze_release_requests(intent)?;
    save_pending_at(path, intent)?;
    while let Some(release) = intent.release_requests.first() {
        crate::replay_v2_request(runtime, "/v2/claim/release", release)?;
        intent.release_requests.remove(0);
        save_pending_at(path, intent)?;
    }
    fs::remove_file(path)?;
    Ok(())
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
                    "intent_id": intent.intent_id.as_deref().ok_or_else(|| anyhow::anyhow!("write authorization is pending recovery"))?,
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

pub(crate) fn replay_pending(
    paths: &GlobalPaths,
    runtime: &ServerRuntime,
    _trigger_repo_root: &Path,
) -> anyhow::Result<()> {
    let root = paths.runtime_dir.join("write-intents");
    let agents = match fs::read_dir(&root) {
        Ok(agents) => agents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let mut failure = None;
    for agent in agents {
        let agent = match agent {
            Ok(agent) => agent,
            Err(error) => {
                failure.get_or_insert_with(|| error.into());
                continue;
            }
        };
        if !agent.file_type()?.is_dir() {
            continue;
        }
        let operations = match fs::read_dir(agent.path()) {
            Ok(operations) => operations,
            Err(error) => {
                failure.get_or_insert_with(|| error.into());
                continue;
            }
        };
        for operation in operations {
            let operation = match operation {
                Ok(operation) => operation,
                Err(error) => {
                    failure.get_or_insert_with(|| error.into());
                    continue;
                }
            };
            let path = operation.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            if let Err(error) = replay_pending_at(runtime, &path) {
                failure.get_or_insert(error);
            }
        }
    }
    failure.map_or(Ok(()), Err)
}

fn replay_pending_at(runtime: &ServerRuntime, path: &Path) -> anyhow::Result<()> {
    let mut intent: PendingIntent = serde_json::from_slice(&fs::read(path)?)?;
    if intent.intent_id.is_none() {
        let response = post_frozen_authorization(runtime, &intent.authorization_request)?;
        if !(200..300).contains(&response.status_code) {
            if serde_json::from_str::<Decision>(&response.body).is_ok() {
                finish_denied_authorization(runtime, path, &mut intent)?;
                return Ok(());
            }
            anyhow::bail!(
                "pending authorization replay failed with HTTP {}",
                response.status_code
            );
        }
        let response: IntentResponse = serde_json::from_str(&response.body)?;
        intent.intent_id = Some(response.intent_id);
        save_pending_at(path, &intent)?;
    }
    if !intent.completed {
        if let Some(request) = intent.completion_request.as_deref() {
            crate::replay_v2_request(runtime, "/v2/write/complete", request)?;
        } else {
            if intent.recovery_request.is_none() {
                let authorization: stateful_core::RequestEnvelope<serde_json::Value> =
                    serde_json::from_str(&intent.authorization_request)?;
                let recovery_root = PathBuf::from(&intent.repo_root);
                if !recovery_root.is_absolute() || intent.repo_root == "unknown" {
                    anyhow::bail!("pending write authorization has no captured absolute repository root");
                }
                let mut request = authorization.clone();
                request.request_id = uuid::Uuid::new_v4();
                request.payload = json!({
                    "intent_id": intent.intent_id.as_deref().ok_or_else(|| anyhow::anyhow!("pending authorization has no intent ID"))?,
                    "actual_fingerprints": intent.targets.iter().map(|path| {
                        Ok(json!({
                            "path": path,
                            "fingerprint": stateful_core::fingerprint_path(&recovery_root.join(path))?,
                        }))
                    }).collect::<anyhow::Result<Vec<_>>>()?,
                });
                intent.recovery_request = Some(serde_json::to_string(&request)?);
                save_pending_at(path, &intent)?;
            }
            let response = crate::replay_v2_request(
                runtime,
                "/v2/write/recover",
                intent.recovery_request.as_deref().expect("recovery request was saved"),
            )?;
            let recovered: serde_json::Value = serde_json::from_str(&response.body)?;
            let operation_id = recovered
                .get("operation_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("write recovery response had no operation ID"))?;
            let authorization: stateful_core::RequestEnvelope<serde_json::Value> =
                serde_json::from_str(&intent.authorization_request)?;
            let expected_operation_id = authorization
                .payload
                .get("operation_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("pending authorization has no operation ID"))?;
            if operation_id != expected_operation_id {
                anyhow::bail!("write recovery operation ID did not match pending authorization");
            }
        }
        intent.completion_request = None;
        intent.recovery_request = None;
        intent.completed = true;
        save_pending_at(path, &intent)?;
    }
        if intent.release_requests.is_empty() {
            freeze_release_requests(&mut intent)?;
            save_pending_at(path, &intent)?;
        }
    while let Some(request) = intent.release_requests.first() {
        crate::replay_v2_request(runtime, "/v2/claim/release", request)?;
        intent.release_requests.remove(0);
        save_pending_at(path, &intent)?;
    }
    fs::remove_file(path)?;
    Ok(())
}

fn freeze_release_requests(intent: &mut PendingIntent) -> anyhow::Result<()> {
    if !intent.release_requests.is_empty() || intent.claim_ids.is_empty() {
        return Ok(());
    }
    let authorization: stateful_core::RequestEnvelope<serde_json::Value> =
        serde_json::from_str(&intent.authorization_request)?;
    intent.release_requests = intent
        .claim_ids
        .iter()
        .map(|claim_id| {
            let mut request = authorization.clone();
            request.request_id = uuid::Uuid::new_v4();
            request.payload = json!({ "claim_id": claim_id });
            serde_json::to_string(&request).map_err(anyhow::Error::from)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(())
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
    save_pending_at(&path, intent)
}

fn save_pending_at(path: &Path, intent: &PendingIntent) -> anyhow::Result<()> {
    let parent = path.parent().ok_or_else(|| anyhow::anyhow!("pending path has no parent"))?;
    let temporary = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name().and_then(|name| name.to_str()).unwrap_or("pending"),
        uuid::Uuid::new_v4()
    ));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    use std::io::Write;
    file.write_all(&serde_json::to_vec(intent)?)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    fs::File::open(parent)?.sync_all()?;
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
    fn atomic_pending_save_replaces_a_corrupt_old_record_without_temp_residue() {
        let temp = tempfile::tempdir().expect("temp dir should create");
        let paths = GlobalPaths::new(temp.path().join("home"));
        let path = pending_path(&paths, "agent-1", "operation-1");
        fs::create_dir_all(path.parent().expect("pending path should have parent"))
            .expect("pending parent should create");
        fs::write(&path, b"{corrupt").expect("corrupt old record should write");
        let pending = PendingIntent {
            intent_id: Some("intent-1".to_string()),
            authorization_request: "{}".to_string(),
            repo_root: "/repo".to_string(),
            targets: vec!["target.txt".to_string()],
            claim_ids: vec!["claim-1".to_string()],
            completion_request: None,
            recovery_request: None,
            release_requests: Vec::new(),
            completed: false,
        };

        save_pending(&paths, "agent-1", "operation-1", &pending)
            .expect("atomic save should replace corrupt old record");

        let persisted: PendingIntent =
            serde_json::from_slice(&fs::read(&path).expect("pending record should read"))
                .expect("pending record should be complete JSON");
        assert_eq!(persisted.intent_id.as_deref(), Some("intent-1"));
        assert!(
            fs::read_dir(path.parent().expect("pending path should have parent"))
                .expect("pending parent should read")
                .all(|entry| {
                    !entry
                        .expect("pending entry should read")
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".operation-1.json.tmp-")
                }),
            "successful atomic save should leave no temporary recovery record"
        );
    }

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

        replay_pending(&paths, &runtime, &repo_root)
            .expect("later lifecycle should replay completion");
        let replay = request_body(&requests.recv().expect("replayed completion should arrive"));
        assert_eq!(replay, initial, "replay must retain the original request UUID");
        assert!(!pending_path(&paths, "agent-1", "operation-1").exists());
    }

    #[test]
    fn retains_each_pending_claim_release_until_all_replay() {
        let temp = tempfile::tempdir().expect("temp dir should create");
        let paths = GlobalPaths::new(temp.path().join("home"));
        let repo_root = temp.path().join("repo");
        fs::create_dir_all(&repo_root).expect("repo should create");
        fs::write(repo_root.join("target.txt"), "after").expect("target should write");
        let (runtime, requests) = fake_server([
            Some(AUTHORIZED),
            Some(r#"{"status":"ok"}"#),
            Some(r#"{"status":"ok"}"#),
            None,
            Some(r#"{"status":"ok"}"#),
        ]);

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
            vec!["claim-1".to_string(), "claim-2".to_string()],
            &SOURCE,
        )
        .expect("authorization should persist its intent");
        let _authorize = requests.recv().expect("authorization should arrive");

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
        .expect("second release failure should retain replay state");
        let _completion = requests.recv().expect("completion should arrive");
        let first_release = request_body(&requests.recv().expect("first release should arrive"));
        let second_release =
            request_body(&requests.recv().expect("second release should arrive"));
        assert_eq!(first_release["payload"]["claim_id"], "claim-1");
        assert_eq!(second_release["payload"]["claim_id"], "claim-2");

        let pending = load_pending(&paths, "agent-1", "operation-1")
            .expect("pending record should load")
            .expect("second release should remain pending");
        assert!(pending.completed);
        assert_eq!(pending.release_requests.len(), 1);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&pending.release_requests[0])
                .expect("pending release should be JSON"),
            second_release,
            "only the second frozen release should remain pending"
        );

        replay_pending(&paths, &runtime, &repo_root)
            .expect("later lifecycle should replay second release");
        let replay = request_body(&requests.recv().expect("replayed release should arrive"));
        assert_eq!(replay, second_release, "claim release replay must retain its request UUID");
        assert!(!pending_path(&paths, "agent-1", "operation-1").exists());
    }

    #[test]
    fn recovery_fingerprints_targets_from_the_authorized_repository_root() {
        let temp = tempfile::tempdir().expect("temp dir should create");
        let paths = GlobalPaths::new(temp.path().join("home"));
        let repo_a = temp.path().join("repo-a");
        let repo_b = temp.path().join("repo-b");
        fs::create_dir_all(&repo_a).expect("repo A should create");
        fs::create_dir_all(&repo_b).expect("repo B should create");
        fs::write(repo_a.join("target.txt"), "repo A").expect("repo A target should write");
        fs::write(repo_b.join("target.txt"), "repo B").expect("repo B target should write");
        let (runtime, requests) = fake_server([
            Some(AUTHORIZED),
            Some(r#"{"intent_id":"intent-1","operation_id":"operation-a"}"#),
        ]);
        let identity = RepoIdentity {
            repo_id: "repo-a".to_string(),
            worktree_id: "worktree-a".to_string(),
            root: repo_a.to_string_lossy().into_owned(),
            branch: "main".to_string(),
        };
        authorize(
            &paths,
            &runtime,
            "agent-1",
            "workspace-1",
            Some(&identity),
            None,
            None,
            "operation-a",
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

        replay_pending(&paths, &runtime, &repo_b).expect("recovery should use captured root");
        let recovery = request_body(&requests.recv().expect("recovery should arrive"));
        assert_eq!(
            recovery["payload"]["actual_fingerprints"][0]["fingerprint"],
            serde_json::to_value(stateful_core::fingerprint_path(&repo_a.join("target.txt")).expect("repo A fingerprint should load"))
                .expect("fingerprint should serialize")
        );
        assert!(!pending_path(&paths, "agent-1", "operation-a").exists());
    }

    #[test]
    fn recovery_without_a_captured_absolute_root_keeps_the_pending_intent() {
        let temp = tempfile::tempdir().expect("temp dir should create");
        let paths = GlobalPaths::new(temp.path().join("home"));
        let repo_a = temp.path().join("repo-a");
        let repo_b = temp.path().join("repo-b");
        fs::create_dir_all(&repo_a).expect("repo A should create");
        fs::create_dir_all(&repo_b).expect("repo B should create");
        fs::write(repo_b.join("target.txt"), "wrong repository").expect("repo B target should write");
        let (runtime, requests) = fake_server([Some(AUTHORIZED)]);
        authorize(
            &paths,
            &runtime,
            "agent-1",
            "workspace-1",
            None,
            None,
            None,
            "operation-unknown-root",
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

        let error = replay_pending(&paths, &runtime, &repo_b)
            .expect_err("unknown root must not fall back to the triggering repository");
        assert!(error.to_string().contains("captured absolute repository root"));
        assert!(pending_path(&paths, "agent-1", "operation-unknown-root").exists());
        assert!(requests.try_recv().is_err(), "no recovery may use repo B");
    }

    #[test]
    fn recovery_without_completion_replays_all_frozen_claim_releases() {
        let temp = tempfile::tempdir().expect("temp dir should create");
        let paths = GlobalPaths::new(temp.path().join("home"));
        let repo_root = temp.path().join("repo");
        fs::create_dir_all(&repo_root).expect("repo should create");
        fs::write(repo_root.join("target.txt"), "after").expect("target should write");
        let (runtime, requests) = fake_server([
            Some(AUTHORIZED),
            Some(r#"{"intent_id":"intent-1","operation_id":"operation-1"}"#),
            Some(r#"{"status":"ok"}"#),
            None,
            Some(r#"{"status":"ok"}"#),
        ]);
        let identity = RepoIdentity {
            repo_id: "repo-1".to_string(),
            worktree_id: "worktree-1".to_string(),
            root: repo_root.to_string_lossy().into_owned(),
            branch: "main".to_string(),
        };

        authorize(
            &paths,
            &runtime,
            "agent-1",
            "workspace-1",
            Some(&identity),
            None,
            None,
            "operation-1",
            "write_file",
            vec![(
                "target.txt".to_string(),
                stateful_core::ContentFingerprint::missing(),
            )],
            None,
            vec!["claim-1".to_string(), "claim-2".to_string()],
            &SOURCE,
        )
        .expect("authorization should persist its intent");
        let _authorize = requests.recv().expect("authorization should arrive");

        assert!(
            replay_pending(&paths, &runtime, &repo_root).is_err(),
            "a lost second release must retain recovery state"
        );
        let recovery = request_body(&requests.recv().expect("recovery should arrive"));
        let first_release = request_body(&requests.recv().expect("first release should arrive"));
        let second_release = request_body(&requests.recv().expect("second release should arrive"));
        assert_eq!(recovery["payload"]["intent_id"], "intent-1");
        assert_eq!(first_release["payload"]["claim_id"], "claim-1");
        assert_eq!(second_release["payload"]["claim_id"], "claim-2");

        let pending = load_pending(&paths, "agent-1", "operation-1")
            .expect("pending record should load")
            .expect("lost release should remain pending");
        assert!(pending.completed);
        assert_eq!(pending.release_requests.len(), 1);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&pending.release_requests[0])
                .expect("pending release should be JSON"),
            second_release,
            "only the failed release remains replayable"
        );

        replay_pending(&paths, &runtime, &repo_root)
            .expect("restart should replay the failed release");
        let replay = request_body(&requests.recv().expect("release replay should arrive"));
        assert_eq!(replay, second_release, "release replay must retain its request UUID");
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
