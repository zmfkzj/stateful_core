use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

use stateful_core::{
    DirectoryTreeState, MutationOperation, ResourceKey, ResourceKind, ResourceObservation,
    ResourceResolver, digest_bytes, digest_canonical_json,
};
use stateful_store::{
    LeaseActivateInput, LeaseRequestState, LeaseRequestStatus, WriteCompleteInput,
    WritePrepareInput, WritePrepareResult, WriteTerminal,
};

use crate::{
    CommandIdentity, ServerRuntime, discover_runtime_with_optional_global, get_payload,
    now_rfc3339, post_command,
    sandbox::{SandboxWritablePath, run_private_sandbox_command},
};

const LEASE_TTL: Duration = Duration::from_secs(60);
const REQUEST_TTL: Duration = Duration::from_secs(120);

pub struct CommitRequest {
    pub repo_root: PathBuf,
    pub message: String,
    pub paths: Vec<String>,
    pub identity: CommandIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CommitResult {
    pub commit_sha: String,
    pub committed_paths: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn run_structured_commit(request: CommitRequest) -> anyhow::Result<CommitResult> {
    let message = request.message.trim();
    if message.is_empty() {
        anyhow::bail!("commit message is required");
    }
    let repo_root = request.repo_root.canonicalize()?;
    if !repo_root.join(".git").exists() {
        anyhow::bail!("structured commit requires a Git repository root");
    }
    if request.identity.task_id.is_empty() || request.identity.agent.agent_id.is_empty() {
        anyhow::bail!("stateful commit requires an active task and agent identity");
    }
    if Path::new(&request.identity.workspace.root).canonicalize()? != repo_root {
        anyhow::bail!("stateful commit identity workspace root does not match repository root");
    }
    let paths = normalize_explicit_paths(&repo_root, &request.paths)?;
    let runtime = discover_runtime_with_optional_global(&repo_root)?;
    let prepared = prepare_commit(&runtime, &request.identity, &repo_root, &paths)?;
    match commit_prepared(&repo_root, message, &paths, &prepared) {
        Ok(mut committed) => {
            let post_resources = committed
                .post
                .as_ref()
                .map(|post| post.resources.clone())
                .unwrap_or_default();
            if let Err(error) = complete(
                &runtime,
                &request.identity,
                &prepared,
                committed.terminal.clone(),
                post_resources.clone(),
                post_resources,
                committed.completion_error.clone(),
            ) {
                committed.result.warnings.push(format!(
                    "server completion after HEAD update failed: {error}"
                ));
            }
            if let Err(error) = apply_hook_changes(
                &runtime,
                &request.identity,
                &repo_root,
                &prepared.batch,
                committed.hook_changes,
            ) {
                committed
                    .result
                    .warnings
                    .push(format!("hook changes after HEAD update failed: {error}"));
            }
            Ok(committed.result)
        }
        Err(error) => complete_failed(&runtime, &request.identity, &repo_root, &prepared, error),
    }
}
fn normalize_explicit_paths(repo_root: &Path, paths: &[String]) -> anyhow::Result<Vec<String>> {
    if paths.is_empty() {
        anyhow::bail!("explicit tracked file paths are required");
    }
    let mut normalized = Vec::new();
    for original in paths {
        let path = original.strip_prefix("./").unwrap_or(original);
        let normalized_path = stateful_core::normalize_relative_path(path);
        if is_broad_pathspec(repo_root, path)
            || is_broad_pathspec(repo_root, &normalized_path)
            || matches_tracked_directory(repo_root, &normalized_path)?
        {
            anyhow::bail!("explicit file paths are required; rejected pathspec `{original}`");
        }
        if !repo_root.join(&normalized_path).exists()
            && !is_tracked_path(repo_root, &normalized_path)?
        {
            anyhow::bail!("explicit file path `{original}` is neither tracked nor present");
        }
        normalized.push(normalized_path);
    }
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

fn is_broad_pathspec(repo_root: &Path, path: &str) -> bool {
    path.is_empty()
        || matches!(path, "." | "*" | ":/")
        || Path::new(path).is_absolute()
        || path.starts_with('-')
        || path.starts_with(':')
        || path.contains("..")
        || path.contains("//")
        || path.ends_with('/')
        || path.contains('\\')
        || path
            .chars()
            .any(|character| matches!(character, '*' | '?' | '[' | ']'))
        || repo_root.join(path).is_dir()
}
fn matches_tracked_directory(repo_root: &Path, path: &str) -> anyhow::Result<bool> {
    Ok(git_stdout(repo_root, &["ls-files", "--", path])?
        .lines()
        .any(|entry| !entry.is_empty() && entry.replace('\\', "/") != path))
}

struct PreparedCommit {
    invocation_id: String,
    attempt_id: String,
    permit_id: String,
    batch: CommitBatch,
}

struct CommittedCommit {
    result: CommitResult,
    post: Option<CommitBatch>,
    terminal: WriteTerminal,
    completion_error: Option<String>,
    hook_changes: BTreeMap<String, Vec<u8>>,
}

struct CommitBatch {
    tracked: BTreeMap<String, Vec<ResourceObservation>>,
    metadata: GitMetadataSnapshot,
    resources: Vec<ResourceObservation>,
}

impl CommitBatch {
    fn workspace_id(&self) -> String {
        self.resources[0].resource().workspace_id.clone()
    }
}

fn prepare_commit(
    runtime: &ServerRuntime,
    identity: &CommandIdentity,
    repo_root: &Path,
    paths: &[String],
) -> anyhow::Result<PreparedCommit> {
    let deadline = Instant::now() + REQUEST_TTL;
    loop {
        let batch = observe_commit_batch(repo_root, &identity.workspace.workspace_id, paths)?;
        stateful_core::validate_operation_start(
            &MutationOperation::StructuredCommit {
                tracked_paths: paths.to_vec(),
            },
            &batch.resources,
        )?;
        record_exact_read(runtime, identity, &batch.resources)?;
        let invocation_id = uuid::Uuid::new_v4().to_string();
        let command = next_identity(identity);
        let result: WritePrepareResult = post_command(
            runtime,
            "/v2/commits/prepare",
            &command,
            &WritePrepareInput {
                invocation_id: invocation_id.clone(),
                operation: MutationOperation::StructuredCommit {
                    tracked_paths: paths.to_vec(),
                },
                current: batch.resources.clone(),
                request_expires_at: future_rfc3339(&command.observed_at, REQUEST_TTL)?,
                lease_expires_at: future_rfc3339(&command.observed_at, LEASE_TTL)?,
                attempt_deadline: future_rfc3339(&command.observed_at, LEASE_TTL)?,
            },
        )?;
        match result {
            WritePrepareResult::Ready {
                attempt_id,
                permit_id,
                ..
            } => {
                return Ok(PreparedCommit {
                    invocation_id,
                    attempt_id,
                    permit_id,
                    batch,
                });
            }
            WritePrepareResult::Queued { batch_id } => {
                wait_for_offer(runtime, identity, repo_root, paths, &batch_id, deadline)?;
            }
            WritePrepareResult::RereadRequired { .. } if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(50));
            }
            WritePrepareResult::RereadRequired { .. } => {
                anyhow::bail!("stateful commit evidence remained stale while waiting for its lease")
            }
            WritePrepareResult::Denied { reason_code } => {
                anyhow::bail!("stateful commit write preparation denied: {reason_code}")
            }
        }
    }
}

fn record_exact_read(
    runtime: &ServerRuntime,
    identity: &CommandIdentity,
    resources: &[ResourceObservation],
) -> anyhow::Result<()> {
    let read_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let _: stateful_store::ReadCommandResult = post_command(
        runtime,
        "/v2/reads/start",
        &next_identity(identity),
        &stateful_store::ReadStartInput {
            read_id: read_id.clone(),
            invocation_id: invocation_id.clone(),
            resources: resources.to_vec(),
        },
    )?;
    let _: stateful_store::ReadCommandResult = post_command(
        runtime,
        "/v2/reads/complete",
        &next_identity(identity),
        &stateful_store::ReadCompleteInput {
            read_id,
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
fn wait_for_offer(
    runtime: &ServerRuntime,
    identity: &CommandIdentity,
    repo_root: &Path,
    paths: &[String],
    initial_batch_id: &str,
    deadline: Instant,
) -> anyhow::Result<()> {
    let mut batch_id = initial_batch_id.to_string();
    while Instant::now() < deadline {
        let query = format!(
            "/v2/lease-requests/{}?workspace_id={}&task_id={}&now={}",
            percent_encode(&batch_id),
            percent_encode(&identity.workspace.workspace_id),
            percent_encode(&identity.task_id),
            percent_encode(&now_rfc3339())
        );
        let status: LeaseRequestStatus = get_payload(runtime, &query)?;
        match status.state {
            LeaseRequestState::Offered => {
                let offer_id = status.offer_id.ok_or_else(|| {
                    anyhow::anyhow!("stateful commit offer is missing its offer id")
                })?;
                let current =
                    observe_commit_batch(repo_root, &identity.workspace.workspace_id, paths)?;
                record_exact_read(runtime, identity, &current.resources)?;
                let command = next_identity(identity);
                let activated: stateful_store::LeaseActivateResult = post_command(
                    runtime,
                    "/v2/leases/activate",
                    &command,
                    &LeaseActivateInput {
                        batch_id: status.batch_id,
                        offer_id,
                        version: status.version,
                        lease_expires_at: future_rfc3339(&command.observed_at, LEASE_TTL)?,
                    },
                )?;
                anyhow::ensure!(
                    activated.active,
                    "stateful commit lease activation was rejected"
                );
                return Ok(());
            }
            LeaseRequestState::Activated => return Ok(()),
            LeaseRequestState::Queued => {}
            LeaseRequestState::Superseded => {
                batch_id = status.superseded_by.ok_or_else(|| {
                    anyhow::anyhow!("superseded commit lease request has no replacement")
                })?;
            }
            LeaseRequestState::Expired | LeaseRequestState::Cancelled => {
                anyhow::bail!("stateful commit lease request is no longer active")
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
    anyhow::bail!("stateful commit lease offer timed out")
}

fn observe_commit_batch(
    repo_root: &Path,
    workspace_id: &str,
    paths: &[String],
) -> anyhow::Result<CommitBatch> {
    let mut tracked = BTreeMap::new();
    let mut resources = Vec::new();
    for path in paths {
        let observations = observe_commit_target(repo_root, workspace_id, path)?;
        resources.extend(observations.clone());
        tracked.insert(path.clone(), observations);
    }
    let git_dir = git_dir(repo_root)?;
    let metadata = GitMetadataSnapshot::capture(repo_root, &git_dir)?;
    resources.push(metadata.observation(workspace_id, &git_dir)?);
    Ok(CommitBatch {
        tracked,
        metadata,
        resources,
    })
}

fn observe_commit_target(
    repo_root: &Path,
    workspace_id: &str,
    path: &str,
) -> anyhow::Result<Vec<ResourceObservation>> {
    let resolver = ResourceResolver::new(workspace_id, repo_root)?;
    match resolver.observe_existing_file(path) {
        Ok(observations) => Ok(observations),
        Err(stateful_core::ResourceError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(resolver
                .absent_entry(path)?
                .into_resources()
                .into_iter()
                .map(|resource| ResourceObservation::Entry {
                    resource,
                    observed: stateful_core::EntryState::Absent,
                    generation: 0,
                })
                .collect())
        }
        Err(error) => Err(error.into()),
    }
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct GitMetadataSnapshot {
    head_sha: Option<String>,
    head: stateful_core::ContentDigest,
    index: Option<stateful_core::ContentDigest>,
    packed_refs: Option<stateful_core::ContentDigest>,
    refs: BTreeMap<String, String>,
}

impl GitMetadataSnapshot {
    fn capture(repo_root: &Path, git_dir: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            head_sha: head_sha(repo_root)?,
            head: digest_bytes(&fs::read(git_dir.join("HEAD"))?),
            index: optional_digest(&git_dir.join("index"))?,
            packed_refs: optional_digest(&git_dir.join("packed-refs"))?,
            refs: git_refs(repo_root)?,
        })
    }

    fn observation(
        &self,
        workspace_id: &str,
        git_dir: &Path,
    ) -> anyhow::Result<ResourceObservation> {
        let (device, inode) = filesystem_identity(&fs::metadata(git_dir)?)?;
        Ok(ResourceObservation::DirectoryTree {
            resource: ResourceKey {
                workspace_id: workspace_id.to_string(),
                kind: ResourceKind::DirectoryTree,
                resource_id: format!("directory:{device}:{inode}"),
                canonical_path: ".git".to_string(),
            },
            observed: DirectoryTreeState::Present {
                device,
                inode,
                snapshot: digest_canonical_json(&serde_json::to_value(self)?),
                entry_count: self.refs.len() as u64 + 3,
            },
            generation: 0,
        })
    }
}

fn optional_digest(path: &Path) -> anyhow::Result<Option<stateful_core::ContentDigest>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(digest_bytes(&bytes))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn git_refs(repo_root: &Path) -> anyhow::Result<BTreeMap<String, String>> {
    let mut refs = BTreeMap::new();
    for line in git_stdout(
        repo_root,
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    )?
    .lines()
    {
        let (name, object) = line
            .split_once(' ')
            .ok_or_else(|| anyhow::anyhow!("invalid Git ref snapshot"))?;
        refs.insert(name.to_string(), object.to_string());
    }
    Ok(refs)
}
fn commit_prepared(
    repo_root: &Path,
    message: &str,
    paths: &[String],
    prepared: &PreparedCommit,
) -> anyhow::Result<CommittedCommit> {
    ensure_batch_unchanged(repo_root, &prepared.batch, paths)?;
    let private = PrivateHookWorkspace::create(repo_root, paths)?;
    let index = TemporaryIndex::from_head(repo_root, &private.root)?;
    stage_private_targets(repo_root, &private.root, &index.path, paths)?;
    let message = TemporaryCommitMessage::create(&private.root, message)?;
    run_allowed_hook(
        repo_root,
        &private.root,
        &index.path,
        "pre-commit",
        &[],
        paths,
    )?;
    stage_private_targets(repo_root, &private.root, &index.path, paths)?;
    run_allowed_hook(
        repo_root,
        &private.root,
        &index.path,
        "commit-msg",
        &[message.path.as_path()],
        paths,
    )?;
    stage_private_targets(repo_root, &private.root, &index.path, paths)?;
    let hook_changes = private.changed_existing_targets()?;
    ensure_batch_unchanged(repo_root, &prepared.batch, paths)?;
    let commit_sha = commit_temp_index_with_head_cas(
        repo_root,
        &index.path,
        &message.path,
        &prepared.batch.metadata.head_sha,
    )?;

    let mut warnings = Vec::new();
    if let Err(error) = synchronize_primary_index(repo_root, &commit_sha, paths) {
        warnings.push(format!(
            "primary index synchronization after HEAD update failed: {error}"
        ));
    }
    let mut completion_warnings = Vec::new();
    let post = match observe_commit_batch(repo_root, &prepared.batch.workspace_id(), paths) {
        Ok(post) => {
            if let Err(error) = stateful_core::validate_operation_transition(
                &MutationOperation::StructuredCommit {
                    tracked_paths: paths.to_vec(),
                },
                &prepared.batch.resources,
                &post.resources,
                &post.resources,
            ) {
                let warning =
                    format!("post-commit observation validation after HEAD update failed: {error}");
                warnings.push(warning.clone());
                completion_warnings.push(warning);
            }
            if let Err(error) = ensure_successful_transition(&prepared.batch, &post) {
                let warning =
                    format!("post-commit transition check after HEAD update failed: {error}");
                warnings.push(warning.clone());
                completion_warnings.push(warning);
            }
            Some(post)
        }
        Err(error) => {
            let warning = format!("post-commit observation after HEAD update failed: {error}");
            warnings.push(warning.clone());
            completion_warnings.push(warning);
            None
        }
    };
    let terminal = if completion_warnings.is_empty() {
        WriteTerminal::Success
    } else {
        WriteTerminal::Uncertain
    };
    Ok(CommittedCommit {
        result: CommitResult {
            commit_sha,
            committed_paths: paths.to_vec(),
            warnings,
        },
        post,
        terminal,
        completion_error: (!completion_warnings.is_empty()).then(|| completion_warnings.join("; ")),
        hook_changes,
    })
}

fn ensure_batch_unchanged(
    repo_root: &Path,
    before: &CommitBatch,
    paths: &[String],
) -> anyhow::Result<()> {
    let current = observe_commit_batch(repo_root, &before.workspace_id(), paths)?;
    if !same_observations(&current.resources, &before.resources) {
        anyhow::bail!(
            "structured commit HEAD, index, ref, or target prestate changed; retry the commit"
        );
    }
    Ok(())
}

fn ensure_successful_transition(before: &CommitBatch, after: &CommitBatch) -> anyhow::Result<()> {
    if before.metadata == after.metadata {
        anyhow::bail!("structured commit did not change Git metadata");
    }
    for (path, start) in &before.tracked {
        if after.tracked.get(path) != Some(start) {
            anyhow::bail!(
                "structured commit changed tracked target `{path}` outside the hook wrapper"
            );
        }
    }
    Ok(())
}
fn complete_failed(
    runtime: &ServerRuntime,
    identity: &CommandIdentity,
    repo_root: &Path,
    prepared: &PreparedCommit,
    error: anyhow::Error,
) -> anyhow::Result<CommitResult> {
    let paths = prepared.batch.tracked.keys().cloned().collect::<Vec<_>>();
    let post = observe_commit_batch(repo_root, &prepared.batch.workspace_id(), &paths);
    let (terminal, resources) = match post {
        Ok(post) if same_observations(&post.resources, &prepared.batch.resources) => {
            (WriteTerminal::FailedKnown, post.resources)
        }
        Ok(post) => (WriteTerminal::Uncertain, post.resources),
        Err(_) => (WriteTerminal::Uncertain, Vec::new()),
    };
    complete(
        runtime,
        identity,
        prepared,
        terminal,
        resources.clone(),
        resources,
        Some(error.to_string()),
    )?;
    Err(error)
}

fn complete(
    runtime: &ServerRuntime,
    identity: &CommandIdentity,
    prepared: &PreparedCommit,
    terminal: WriteTerminal,
    post_resources: Vec<ResourceObservation>,
    expected_post_resources: Vec<ResourceObservation>,
    error: Option<String>,
) -> anyhow::Result<()> {
    let result: stateful_store::WriteCompleteResult = post_command(
        runtime,
        "/v2/commits/complete",
        &next_identity(identity),
        &WriteCompleteInput {
            attempt_id: prepared.attempt_id.clone(),
            permit_id: prepared.permit_id.clone(),
            invocation_id: prepared.invocation_id.clone(),
            terminal: terminal.clone(),
            post_resources,
            expected_post_resources,
            error,
        },
    )?;
    let expected = match terminal {
        WriteTerminal::Success => stateful_store::WriteResultStatus::Completed,
        WriteTerminal::FailedKnown => stateful_store::WriteResultStatus::Failed,
        WriteTerminal::Uncertain => stateful_store::WriteResultStatus::Uncertain,
    };
    anyhow::ensure!(
        result.status == expected,
        "stateful commit completion became {:?}",
        result.status
    );
    Ok(())
}
fn apply_hook_changes(
    runtime: &ServerRuntime,
    identity: &CommandIdentity,
    repo_root: &Path,
    before: &CommitBatch,
    changes: BTreeMap<String, Vec<u8>>,
) -> anyhow::Result<()> {
    for (path, contents) in changes {
        let current = observe_commit_target(repo_root, &identity.workspace.workspace_id, &path)?;
        if before.tracked.get(&path) != Some(&current) {
            anyhow::bail!("hook target `{path}` changed before wrapper CAS could apply it");
        }
        record_exact_read(runtime, identity, &current)?;
        let invocation_id = uuid::Uuid::new_v4().to_string();
        let command = next_identity(identity);
        let result: WritePrepareResult = post_command(
            runtime,
            "/v2/writes/prepare",
            &command,
            &WritePrepareInput {
                invocation_id: invocation_id.clone(),
                operation: MutationOperation::Update { path: path.clone() },
                current: current.clone(),
                request_expires_at: future_rfc3339(&command.observed_at, REQUEST_TTL)?,
                lease_expires_at: future_rfc3339(&command.observed_at, LEASE_TTL)?,
                attempt_deadline: future_rfc3339(&command.observed_at, LEASE_TTL)?,
            },
        )?;
        let WritePrepareResult::Ready {
            attempt_id,
            permit_id,
            ..
        } = result
        else {
            anyhow::bail!("hook target `{path}` could not acquire its commit lease");
        };
        let write = fs::write(repo_root.join(&path), contents);
        let post = observe_commit_target(repo_root, &identity.workspace.workspace_id, &path);
        let (terminal, actual, expected, error) = match (write, post) {
            (Ok(()), Ok(post)) => (WriteTerminal::Success, post.clone(), post, None),
            (Err(error), Ok(post)) if post == current => (
                WriteTerminal::FailedKnown,
                post.clone(),
                post,
                Some(error.to_string()),
            ),
            (Err(error), Ok(post)) => (
                WriteTerminal::Uncertain,
                post.clone(),
                post,
                Some(error.to_string()),
            ),
            (Err(error), Err(_)) => (
                WriteTerminal::Uncertain,
                Vec::new(),
                Vec::new(),
                Some(error.to_string()),
            ),
            (Ok(()), Err(error)) => (
                WriteTerminal::Uncertain,
                Vec::new(),
                Vec::new(),
                Some(error.to_string()),
            ),
        };
        let _: stateful_store::WriteCompleteResult = post_command(
            runtime,
            "/v2/writes/complete",
            &next_identity(identity),
            &WriteCompleteInput {
                attempt_id,
                permit_id,
                invocation_id,
                terminal,
                post_resources: actual,
                expected_post_resources: expected,
                error: error.clone(),
            },
        )?;
        if let Some(error) = error {
            anyhow::bail!("hook target `{path}` wrapper apply failed: {error}");
        }
    }
    Ok(())
}
struct PrivateHookWorkspace {
    root: PathBuf,
    initial: BTreeMap<String, Option<Vec<u8>>>,
}

impl PrivateHookWorkspace {
    fn create(repo_root: &Path, paths: &[String]) -> anyhow::Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "stateful-commit-hooks-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&root)?;
        let root = root.canonicalize().inspect_err(|_| {
            let _ = fs::remove_dir_all(&root);
        })?;
        let result = (|| {
            if root.to_str().is_none() {
                anyhow::bail!("private hook workspace path must be valid UTF-8");
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
            }
            if root.canonicalize()?.starts_with(repo_root) {
                anyhow::bail!("private hook workspace must not be inside the repository");
            }
            fs::create_dir(root.join(".stateful-tmp"))?;
            let mut initial = BTreeMap::new();
            for path in paths {
                let source = repo_root.join(path);
                match fs::read(&source) {
                    Ok(contents) => {
                        let target = root.join(path);
                        if let Some(parent) = target.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        fs::write(&target, &contents)?;
                        fs::set_permissions(&target, fs::metadata(source)?.permissions())?;
                        initial.insert(path.clone(), Some(contents));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        initial.insert(path.clone(), None);
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Ok(Self {
                root: root.clone(),
                initial,
            })
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&root);
        }
        result
    }

    fn changed_existing_targets(&self) -> anyhow::Result<BTreeMap<String, Vec<u8>>> {
        let mut changed = BTreeMap::new();
        for (path, before) in &self.initial {
            let target = self.root.join(path);
            match before {
                Some(before) => {
                    let metadata = fs::symlink_metadata(&target).map_err(|_| {
                        anyhow::anyhow!(
                            "hook changed declared target `{path}` away from a regular file"
                        )
                    })?;
                    if !metadata.is_file() || metadata.file_type().is_symlink() {
                        anyhow::bail!(
                            "hook changed declared target `{path}` away from a regular file"
                        );
                    }
                    let after = fs::read(target)?;
                    if &after != before {
                        changed.insert(path.clone(), after);
                    }
                }
                None if target.exists() => {
                    anyhow::bail!(
                        "hook created declared absent target `{path}`; use a V2 create before commit"
                    );
                }
                None => {}
            }
        }
        Ok(changed)
    }
}

impl Drop for PrivateHookWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn stage_private_targets(
    repo_root: &Path,
    private_root: &Path,
    index: &Path,
    paths: &[String],
) -> anyhow::Result<()> {
    for path in paths {
        let mut command = Command::new("git");
        if private_root.join(path).is_file() {
            command.args(["add", "--force", "--", path]);
        } else {
            command.args(["update-index", "--force-remove", "--", path]);
        }
        command.current_dir(private_root);
        sanitize_git_environment(&mut command);
        command
            .env("GIT_DIR", git_dir(repo_root)?)
            .env("GIT_WORK_TREE", private_root)
            .env("GIT_INDEX_FILE", index);
        let output = command.output()?;
        if !output.status.success() {
            anyhow::bail!(
                "git private stage failed for `{path}`: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }
    Ok(())
}
fn run_allowed_hook(
    repo_root: &Path,
    private_root: &Path,
    index: &Path,
    name: &str,
    args: &[&Path],
    explicit: &[String],
) -> anyhow::Result<()> {
    let Some(hook) = hook_path(repo_root, name)? else {
        return Ok(());
    };
    let git_dir = git_dir(repo_root)?;
    let mut command = format!(
        "GIT_DIR={} GIT_WORK_TREE={} GIT_INDEX_FILE={} exec {}",
        posix_shell_quote_path(&git_dir, "hook GIT_DIR")?,
        posix_shell_quote_path(private_root, "hook GIT_WORK_TREE")?,
        posix_shell_quote_path(index, "hook GIT_INDEX_FILE")?,
        posix_shell_quote_path(&hook, "hook path")?,
    );
    let mut writable_paths = vec![SandboxWritablePath::directory(private_root.to_path_buf())];
    for argument in args {
        command.push(' ');
        command.push_str(&posix_shell_quote_path(argument, "hook argument")?);
        writable_paths.push(SandboxWritablePath::file((*argument).to_path_buf()));
    }
    let before = HookBoundary::capture(repo_root, index)?;
    let output = run_private_sandbox_command(&command, private_root, &writable_paths, LEASE_TTL)
        .map_err(|error| anyhow::anyhow!("{name} hook sandbox failed: {error}"))?;
    before.assert_only_declared(&HookBoundary::capture(repo_root, index)?, explicit)?;
    if output.status != "exited" || output.exit_code != Some(0) {
        anyhow::bail!(
            "{name} hook failed ({}, {:?}): {}",
            output.status,
            output.exit_code,
            output.stderr.trim()
        );
    }
    Ok(())
}

fn posix_shell_quote_path(path: &Path, field: &str) -> anyhow::Result<String> {
    let value = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("{field} must be valid UTF-8"))?;
    if value.contains('\0') {
        anyhow::bail!("{field} must not contain a NUL byte");
    }
    Ok(format!("'{}'", value.replace('\'', "'\"'\"'")))
}

fn hook_path(repo_root: &Path, name: &str) -> anyhow::Result<Option<PathBuf>> {
    let path = format!("hooks/{name}");
    let hook = PathBuf::from(git_stdout(repo_root, &["rev-parse", "--git-path", &path])?.trim());
    let hook = if hook.is_absolute() {
        hook
    } else {
        repo_root.join(hook)
    };
    if !hook.is_file() {
        return Ok(None);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(&hook)?.permissions().mode() & 0o111 == 0 {
            return Ok(None);
        }
    }
    Ok(Some(hook))
}

struct HookBoundary {
    worktree_status: Vec<u8>,
    metadata: GitMetadataSnapshot,
    index: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl HookBoundary {
    fn capture(repo_root: &Path, index: &Path) -> anyhow::Result<Self> {
        let git_dir = git_dir(repo_root)?;
        Ok(Self {
            worktree_status: git_stdout_bytes_with_index(
                repo_root,
                &["status", "--porcelain=v1", "-z", "-uall"],
                None,
            )?,
            metadata: GitMetadataSnapshot::capture(repo_root, &git_dir)?,
            index: index_snapshot(repo_root, index)?,
        })
    }

    fn assert_only_declared(&self, after: &Self, explicit: &[String]) -> anyhow::Result<()> {
        if self.worktree_status != after.worktree_status {
            anyhow::bail!("hook wrote to the real workspace outside its private staging area");
        }
        if self.metadata != after.metadata {
            anyhow::bail!("hook modified Git HEAD, refs, or the primary index");
        }
        let allowed = explicit
            .iter()
            .map(|path| path.as_bytes().to_vec())
            .collect::<BTreeSet<_>>();
        for path in changed_bytes_keys(&self.index, &after.index) {
            if !allowed.contains(&path) {
                anyhow::bail!(
                    "hook modified undeclared index path `{}`",
                    String::from_utf8_lossy(&path)
                );
            }
        }
        Ok(())
    }
}

fn index_snapshot(repo_root: &Path, index: &Path) -> anyhow::Result<BTreeMap<Vec<u8>, Vec<u8>>> {
    let bytes =
        git_stdout_bytes_with_index(repo_root, &["ls-files", "--stage", "-z"], Some(index))?;
    let mut snapshot = BTreeMap::new();
    for entry in bytes
        .split(|byte| *byte == b'\0')
        .filter(|entry| !entry.is_empty())
    {
        let separator = entry
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or_else(|| anyhow::anyhow!("invalid Git index entry"))?;
        let (metadata, path) = entry.split_at(separator);
        snapshot.insert(path[1..].to_vec(), metadata.to_vec());
    }
    Ok(snapshot)
}

fn changed_bytes_keys<T: PartialEq>(
    before: &BTreeMap<Vec<u8>, T>,
    after: &BTreeMap<Vec<u8>, T>,
) -> Vec<Vec<u8>> {
    before
        .keys()
        .chain(after.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| before.get(*path) != after.get(*path))
        .cloned()
        .collect()
}

struct TemporaryIndex {
    path: PathBuf,
}

impl TemporaryIndex {
    fn from_head(repo_root: &Path, private_root: &Path) -> anyhow::Result<Self> {
        let path = private_root.join(format!(
            "stateful-commit-index-{}-{}.index",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        match head_sha(repo_root)? {
            Some(_) => git_status_with_index(repo_root, &["read-tree", "HEAD"], Some(&path))?,
            None => git_status_with_index(repo_root, &["read-tree", "--empty"], Some(&path))?,
        }
        Ok(Self { path })
    }
}

impl Drop for TemporaryIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(self.path.with_extension("index.lock"));
    }
}

struct TemporaryCommitMessage {
    path: PathBuf,
}

impl TemporaryCommitMessage {
    fn create(private_root: &Path, message: &str) -> anyhow::Result<Self> {
        let path = private_root.join(format!(
            "stateful-commit-message-{}-{}.txt",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::write(&path, format!("{message}\n"))?;
        Ok(Self { path })
    }
}

impl Drop for TemporaryCommitMessage {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
fn commit_temp_index_with_head_cas(
    repo_root: &Path,
    index: &Path,
    message: &Path,
    expected_head: &Option<String>,
) -> anyhow::Result<String> {
    if head_sha(repo_root)?.as_ref() != expected_head.as_ref() {
        anyhow::bail!("structured commit HEAD prestate changed; retry the commit");
    }
    let tree = git_stdout_with_index(repo_root, &["write-tree"], Some(index))?
        .trim()
        .to_string();
    let mut commit_args = vec!["commit-tree", tree.as_str()];
    if let Some(head) = expected_head {
        commit_args.extend(["-p", head]);
    }
    let message = message.to_string_lossy().to_string();
    commit_args.extend(["-F", message.as_str()]);
    let commit_sha = git_stdout(repo_root, &commit_args)?.trim().to_string();
    update_head_cas(repo_root, &commit_sha, expected_head)?;
    Ok(commit_sha)
}

fn update_head_cas(
    repo_root: &Path,
    commit_sha: &str,
    expected_head: &Option<String>,
) -> anyhow::Result<()> {
    let mut update_args = vec!["update-ref", "HEAD", commit_sha];
    if let Some(head) = expected_head {
        update_args.push(head);
    } else {
        update_args.push("");
    }
    git_status(repo_root, &update_args)
}

fn synchronize_primary_index(
    repo_root: &Path,
    commit_sha: &str,
    paths: &[String],
) -> anyhow::Result<()> {
    let mut args = vec!["reset", "--quiet", commit_sha, "--"];
    args.extend(paths.iter().map(String::as_str));
    git_status(repo_root, &args)
}

fn git_dir(repo_root: &Path) -> anyhow::Result<PathBuf> {
    Ok(PathBuf::from(
        git_stdout(repo_root, &["rev-parse", "--absolute-git-dir"])?.trim(),
    ))
}

fn head_sha(repo_root: &Path) -> anyhow::Result<Option<String>> {
    let output = git_command(repo_root, &["rev-parse", "--verify", "HEAD"], None).output()?;
    if output.status.success() {
        return Ok(Some(String::from_utf8(output.stdout)?.trim().to_string()));
    }
    Ok(None)
}

fn is_tracked_path(repo_root: &Path, path: &str) -> anyhow::Result<bool> {
    Ok(git_command(
        repo_root,
        &["ls-files", "--error-unmatch", "--", path],
        None,
    )
    .output()?
    .status
    .success())
}

fn git_status(repo_root: &Path, args: &[&str]) -> anyhow::Result<()> {
    git_status_with_index(repo_root, args, None)
}

fn git_status_with_index(
    repo_root: &Path,
    args: &[&str],
    index: Option<&Path>,
) -> anyhow::Result<()> {
    let output = git_command(repo_root, args, index).output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn git_stdout(repo_root: &Path, args: &[&str]) -> anyhow::Result<String> {
    String::from_utf8(git_stdout_bytes_with_index(repo_root, args, None)?).map_err(Into::into)
}

fn git_stdout_with_index(
    repo_root: &Path,
    args: &[&str],
    index: Option<&Path>,
) -> anyhow::Result<String> {
    String::from_utf8(git_stdout_bytes_with_index(repo_root, args, index)?).map_err(Into::into)
}

fn git_stdout_bytes_with_index(
    repo_root: &Path,
    args: &[&str],
    index: Option<&Path>,
) -> anyhow::Result<Vec<u8>> {
    let output = git_command(repo_root, args, index).output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}
fn git_command(repo_root: &Path, args: &[&str], index: Option<&Path>) -> Command {
    let mut command = Command::new("git");
    command.args(args).current_dir(repo_root);
    sanitize_git_environment(&mut command);
    command.env("GIT_OPTIONAL_LOCKS", "0");
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    command
}

fn sanitize_git_environment(command: &mut Command) {
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_EXTERNAL_DIFF",
        "GIT_PAGER",
    ] {
        command.env_remove(key);
    }
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("GIT_CONFIG_") || name.starts_with("GIT_TRACE") {
            command.env_remove(key);
        }
    }
}

fn same_observations(left: &[ResourceObservation], right: &[ResourceObservation]) -> bool {
    let mut left = left.to_vec();
    let mut right = right.to_vec();
    let key = |value: &ResourceObservation| {
        let resource = value.resource();
        (
            resource.workspace_id.clone(),
            resource.kind,
            resource.resource_id.clone(),
            resource.canonical_path.clone(),
        )
    };
    left.sort_by_key(key);
    right.sort_by_key(key);
    left == right
}

fn next_identity(identity: &CommandIdentity) -> CommandIdentity {
    CommandIdentity::new(
        identity.task_id.clone(),
        uuid::Uuid::new_v4().to_string(),
        now_rfc3339(),
        identity.agent.clone(),
        identity.workspace.clone(),
        identity.source.clone(),
    )
}

fn future_rfc3339(observed_at: &str, after: Duration) -> anyhow::Result<String> {
    use time::{Duration as TimeDuration, OffsetDateTime, format_description::well_known::Rfc3339};
    let observed_at = OffsetDateTime::parse(observed_at, &Rfc3339)?;
    let seconds = i64::try_from(after.as_secs())?;
    Ok((observed_at + TimeDuration::seconds(seconds)).format(&Rfc3339)?)
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

#[cfg(unix)]
fn filesystem_identity(metadata: &fs::Metadata) -> anyhow::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn filesystem_identity(_: &fs::Metadata) -> anyhow::Result<(u64, u64)> {
    anyhow::bail!("filesystem identity is unsupported on this platform")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn hook_shell_paths_fail_closed_for_non_utf8() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};

        let path = PathBuf::from(OsString::from_vec(vec![b'a', 0xff]));
        assert!(posix_shell_quote_path(&path, "hook path").is_err());
    }

    #[test]
    fn update_head_cas_rejects_a_changed_head() {
        let root = tempfile::tempdir().expect("test operation should succeed");
        git(root.path(), &["init"]);
        git(root.path(), &["config", "user.name", "stateful test"]);
        git(
            root.path(),
            &["config", "user.email", "stateful@example.invalid"],
        );
        fs::write(root.path().join("file"), "base\n").expect("test operation should succeed");
        git(root.path(), &["add", "file"]);
        git(root.path(), &["commit", "-m", "seed"]);
        let expected = head_sha(root.path()).expect("test operation should succeed");
        git(root.path(), &["commit", "--allow-empty", "-m", "advance"]);

        let error = update_head_cas(
            root.path(),
            expected.as_deref().expect("test operation should succeed"),
            &expected,
        )
        .expect_err("test operation should fail");
        assert!(error.to_string().contains("git update-ref HEAD"));
    }

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("test operation should succeed");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
