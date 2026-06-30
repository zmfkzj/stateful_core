use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use stateful_core::normalize_relative_path;

use crate::{
    GlobalPaths, ProtocolEnvelopeArgs, RepoIdentity, discover_runtime_with_optional_global,
    effective_workspace_id_for_repo, post_json, protocol_envelope, repo_identity_for_enabled_repo,
};

pub type AuthorizePath = Box<dyn Fn(&str, &str) -> anyhow::Result<()> + Send + Sync>;

pub struct CommitRequest {
    pub repo_root: PathBuf,
    pub message: String,
    pub paths: Vec<String>,
    pub agent_id: Option<String>,
    pub workspace_id: Option<String>,
    pub authorize: Option<AuthorizePath>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CommitResult {
    pub commit_sha: String,
    pub committed_paths: Vec<String>,
}

pub fn run_structured_commit(request: CommitRequest) -> anyhow::Result<CommitResult> {
    let message = request.message.trim();
    if message.is_empty() {
        anyhow::bail!("commit message is required");
    }

    let paths = normalize_explicit_paths(&request.repo_root, &request.paths)?;
    let explicit = paths.iter().cloned().collect::<BTreeSet<_>>();

    deny_unrelated_staged_changes(&request.repo_root, &explicit)?;
    reject_rename_status(&request.repo_root, &paths)?;
    let targets = paths
        .iter()
        .map(|path| commit_target(&request.repo_root, path))
        .collect::<anyhow::Result<Vec<_>>>()?;

    for target in &targets {
        authorize_path(&request, target)?;
    }

    let temporary_index = TemporaryIndex::create(&request.repo_root)?;
    let commit_message = TemporaryCommitMessage::create(message)?;
    let disabled_hooks = TemporaryHooksDir::create()?;
    let result = (|| -> anyhow::Result<CommitResult> {
        let mut add_args = vec!["add", "--force", "--"];
        add_args.extend(paths.iter().map(String::as_str));
        git_status_with_index(&request.repo_root, &add_args, Some(&temporary_index.path))?;

        run_commit_hooks_with_index(
            &request.repo_root,
            &temporary_index.path,
            &commit_message.path,
        )?;
        revalidate_staged_targets(&request.repo_root, &targets, &temporary_index.path)?;
        deny_unrelated_staged_changes_with_index(
            &request.repo_root,
            &explicit,
            Some(&temporary_index.path),
        )?;
        reject_rename_status_with_index(&request.repo_root, &paths, Some(&temporary_index.path))?;

        let hooks_config = format!("core.hooksPath={}", disabled_hooks.path.to_string_lossy());
        let message_path = commit_message.path.to_string_lossy().to_string();
        let commit_args = vec![
            "-c",
            hooks_config.as_str(),
            "commit",
            "--no-verify",
            "-F",
            message_path.as_str(),
        ];
        git_status_with_index(
            &request.repo_root,
            &commit_args,
            Some(&temporary_index.path),
        )?;
        run_post_commit_hook_with_index(&request.repo_root, &temporary_index.path);

        let commit_sha = git_stdout(&request.repo_root, &["rev-parse", "HEAD"])?;
        Ok(CommitResult {
            commit_sha: commit_sha.trim().to_string(),
            committed_paths: paths.clone(),
        })
    })();

    match result {
        Ok(result) => {
            restore_committed_paths_to_head(&request.repo_root, &paths)?;
            Ok(result)
        }
        Err(error) => Err(error),
    }
}

fn normalize_explicit_paths(repo_root: &Path, paths: &[String]) -> anyhow::Result<Vec<String>> {
    if paths.is_empty() {
        anyhow::bail!("explicit file paths are required");
    }

    let mut normalized = Vec::new();
    for path in paths {
        let original_path = path.as_str();
        let path = original_path.strip_prefix("./").unwrap_or(original_path);
        if is_broad_pathspec(repo_root, path) {
            anyhow::bail!("explicit file paths are required; rejected pathspec `{original_path}`");
        }
        let normalized_path = normalize_relative_path(path);
        if is_broad_pathspec(repo_root, &normalized_path) {
            anyhow::bail!("explicit file paths are required; rejected pathspec `{original_path}`");
        }
        if matches_tracked_directory(repo_root, &normalized_path)? {
            anyhow::bail!("explicit file paths are required; rejected pathspec `{original_path}`");
        }
        normalized.push(normalized_path);
    }

    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

fn is_broad_pathspec(repo_root: &Path, path: &str) -> bool {
    path.is_empty()
        || path == "."
        || path == "*"
        || path == ":/"
        || Path::new(path).is_absolute()
        || path.starts_with('-')
        || path.starts_with(':')
        || path.contains("..")
        || path.contains("//")
        || path.ends_with('/')
        || path.contains('\\')
        || path.contains('*')
        || path.contains('?')
        || path.contains('[')
        || path.contains(']')
        || repo_root.join(path).is_dir()
}

fn matches_tracked_directory(repo_root: &Path, path: &str) -> anyhow::Result<bool> {
    let tracked = git_stdout(repo_root, &["ls-files", "--", path])?;
    Ok(tracked
        .lines()
        .filter(|line| !line.trim().is_empty())
        .any(|line| line.replace('\\', "/") != path))
}

fn deny_unrelated_staged_changes(
    repo_root: &Path,
    explicit: &BTreeSet<String>,
) -> anyhow::Result<()> {
    deny_unrelated_staged_changes_with_index(repo_root, explicit, None)
}

fn deny_unrelated_staged_changes_with_index(
    repo_root: &Path,
    explicit: &BTreeSet<String>,
    index_path: Option<&Path>,
) -> anyhow::Result<()> {
    let staged =
        git_stdout_with_index(repo_root, &["diff", "--cached", "--name-only"], index_path)?;
    let unrelated = staged
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.replace('\\', "/"))
        .filter(|path| !explicit.contains(path))
        .collect::<Vec<_>>();

    if !unrelated.is_empty() {
        let hook_context = if index_path.is_some() {
            "; hook modified the worktree or staged unrelated paths"
        } else {
            ""
        };
        anyhow::bail!(
            "unrelated staged changes are present: {}{}",
            unrelated.join(", "),
            hook_context
        );
    }

    Ok(())
}

fn authorize_path(request: &CommitRequest, target: &CommitTarget) -> anyhow::Result<()> {
    if let Some(authorize) = &request.authorize {
        return authorize(target.action, &target.path);
    }

    let agent_id = request
        .agent_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("stateful commit requires a current agent id"))?;
    let runtime = discover_runtime_with_optional_global(&request.repo_root)?;
    let identity = GlobalPaths::from_env()
        .ok()
        .and_then(|paths| repo_identity_for_enabled_repo(&paths, &request.repo_root).ok());
    let workspace_id = commit_workspace_id(
        request.workspace_id.as_deref(),
        runtime.workspace_id.as_str(),
        identity.as_ref(),
    );
    let body = protocol_envelope(ProtocolEnvelopeArgs {
        runtime: &runtime,
        request_id: uuid::Uuid::new_v4().to_string(),
        agent_id: agent_id.to_string(),
        workspace_id,
        identity,
        source_kind: "cli",
        event: "commit_authorize",
        source_ref: "stateful-commit",
        source_tool_name: None,
        payload: serde_json::json!({
            "action": target.action,
            "path": target.path,
        }),
    });
    let response = post_json(&runtime, "/v1/authorize", &body)?;

    if !(200..300).contains(&response.status_code) {
        anyhow::bail!(
            "stateful commit authorization failed with HTTP {}: {}",
            response.status_code,
            response.body
        );
    }

    let decision = serde_json::from_str::<CommitAuthorizeDecision>(&response.body)?;
    if decision.decision != "allow" {
        anyhow::bail!(
            "{}",
            decision.required_next_action.unwrap_or(decision.message)
        );
    }

    Ok(())
}

fn commit_workspace_id(
    explicit_workspace_id: Option<&str>,
    runtime_workspace_id: &str,
    identity: Option<&RepoIdentity>,
) -> String {
    if let Some(workspace_id) = explicit_workspace_id {
        return workspace_id.to_string();
    }
    effective_workspace_id_for_repo(runtime_workspace_id, identity)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommitTarget {
    path: String,
    action: &'static str,
}

fn commit_target(repo_root: &Path, path: &str) -> anyhow::Result<CommitTarget> {
    let action = if is_missing_tracked_file(repo_root, path)? {
        "delete_file"
    } else {
        "write_file"
    };

    Ok(CommitTarget {
        path: path.to_string(),
        action,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo_registry::RepoIdentity;

    #[test]
    fn commit_authorization_defaults_local_runtime_to_repo_workspace() {
        let identity = RepoIdentity {
            repo_id: "repo-abc123".to_string(),
            worktree_id: "repo-abc123".to_string(),
            root: "/repo".to_string(),
            branch: "main".to_string(),
        };

        assert_eq!(
            commit_workspace_id(None, "local", Some(&identity)),
            "workspace-abc123"
        );
    }

    #[test]
    fn commit_authorization_preserves_explicit_workspace_and_derives_default_remote_workspace() {
        let identity = RepoIdentity {
            repo_id: "repo-abc123".to_string(),
            worktree_id: "repo-abc123".to_string(),
            root: "/repo".to_string(),
            branch: "main".to_string(),
        };

        assert_eq!(
            commit_workspace_id(Some("manual"), "local", Some(&identity)),
            "manual"
        );
        assert_eq!(
            commit_workspace_id(None, "shared", Some(&identity)),
            "workspace-abc123"
        );
    }
}

fn is_missing_tracked_file(repo_root: &Path, path: &str) -> anyhow::Result<bool> {
    if repo_root.join(path).exists() {
        return Ok(false);
    }

    Ok(git_command(
        repo_root,
        &["ls-files", "--error-unmatch", "--", path],
        None,
    )
    .output()?
    .status
    .success())
}

fn revalidate_staged_targets(
    repo_root: &Path,
    authorized_targets: &[CommitTarget],
    index_path: &Path,
) -> anyhow::Result<()> {
    for authorized in authorized_targets {
        let staged = staged_commit_target(repo_root, &authorized.path, index_path)?;
        if staged.action != authorized.action {
            anyhow::bail!(
                "stateful commit target `{}` changed from {} to {} after authorization; retry the commit",
                authorized.path,
                authorized.action,
                staged.action
            );
        }
    }
    Ok(())
}

fn staged_commit_target(
    repo_root: &Path,
    path: &str,
    index_path: &Path,
) -> anyhow::Result<CommitTarget> {
    let staged_entry = git_stdout_with_index(
        repo_root,
        &["ls-files", "--stage", "--", path],
        Some(index_path),
    )?;
    let action = if staged_entry.trim().is_empty() {
        "delete_file"
    } else {
        "write_file"
    };

    Ok(CommitTarget {
        path: path.to_string(),
        action,
    })
}

fn run_commit_hooks_with_index(
    repo_root: &Path,
    index_path: &Path,
    message_path: &Path,
) -> anyhow::Result<()> {
    run_git_hook_with_index(repo_root, index_path, "pre-commit", &[], true)?;
    run_git_hook_with_index(
        repo_root,
        index_path,
        "prepare-commit-msg",
        &[message_path, Path::new("message")],
        true,
    )?;
    run_git_hook_with_index(repo_root, index_path, "commit-msg", &[message_path], true)?;
    Ok(())
}

fn run_post_commit_hook_with_index(repo_root: &Path, index_path: &Path) {
    let _ = run_git_hook_with_index(repo_root, index_path, "post-commit", &[], false);
}

fn run_git_hook_with_index(
    repo_root: &Path,
    index_path: &Path,
    hook_name: &str,
    args: &[&Path],
    restore_worktree: bool,
) -> anyhow::Result<()> {
    let hook_rev_path = format!("hooks/{hook_name}");
    let hook_path = git_stdout(
        repo_root,
        &["rev-parse", "--git-path", hook_rev_path.as_str()],
    )?;
    let hook_path = PathBuf::from(hook_path.trim());
    let hook_path = if hook_path.is_absolute() {
        hook_path
    } else {
        repo_root.join(hook_path)
    };
    if !hook_path.is_file() {
        return Ok(());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&hook_path)?.permissions().mode();
        if mode & 0o111 == 0 {
            return Ok(());
        }
    }

    let snapshot = if restore_worktree {
        Some(WorktreeSnapshot::capture(repo_root)?)
    } else {
        None
    };
    let mut command = Command::new(&hook_path);
    command.current_dir(repo_root);
    sanitize_git_environment(&mut command);
    let git_dir = git_stdout(repo_root, &["rev-parse", "--absolute-git-dir"])?;
    command.env("GIT_DIR", git_dir.trim());
    command.env("GIT_WORK_TREE", repo_root);
    command.env("GIT_INDEX_FILE", index_path);
    command.args(args);
    let output = command.output()?;
    if let Some(snapshot) = snapshot {
        snapshot.restore(repo_root)?;
    }
    if !output.status.success() {
        anyhow::bail!(
            "{hook_name} hook failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

struct WorktreeSnapshot {
    paths: BTreeMap<String, Option<Vec<u8>>>,
}

impl WorktreeSnapshot {
    fn capture(repo_root: &Path) -> anyhow::Result<Self> {
        let status = git_stdout(repo_root, &["status", "--porcelain=v1", "-z", "-uall"])?;
        let mut paths = BTreeMap::new();
        for path in porcelain_status_paths(&status) {
            let contents = fs::read(repo_root.join(&path)).ok();
            paths.insert(path, contents);
        }
        Ok(Self { paths })
    }

    fn restore(self, repo_root: &Path) -> anyhow::Result<()> {
        let status = git_stdout(repo_root, &["status", "--porcelain=v1", "-z", "-uall"])?;
        for path in porcelain_status_paths(&status) {
            if let Some(contents) = self.paths.get(&path) {
                restore_snapshot_path(repo_root, &path, contents.as_deref())?;
            } else if is_tracked_path(repo_root, &path)? {
                git_status(repo_root, &["checkout", "--", path.as_str()])?;
            } else {
                remove_worktree_path(&repo_root.join(&path))?;
            }
        }
        Ok(())
    }
}

fn porcelain_status_paths(status: &str) -> BTreeSet<String> {
    status
        .split('\0')
        .filter(|entry| entry.len() > 3)
        .map(|entry| entry[3..].replace('\\', "/"))
        .collect()
}

fn restore_snapshot_path(
    repo_root: &Path,
    path: &str,
    contents: Option<&[u8]>,
) -> anyhow::Result<()> {
    let target = repo_root.join(path);
    if let Some(contents) = contents {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(target, contents)?;
    } else {
        remove_worktree_path(&target)?;
    }
    Ok(())
}

fn remove_worktree_path(path: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path)?,
        Ok(_) => fs::remove_file(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
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

fn reject_rename_status(repo_root: &Path, paths: &[String]) -> anyhow::Result<()> {
    reject_rename_status_with_index(repo_root, paths, None)
}

fn reject_rename_status_with_index(
    repo_root: &Path,
    paths: &[String],
    index_path: Option<&Path>,
) -> anyhow::Result<()> {
    let mut args = vec!["status", "--porcelain=v1", "--find-renames", "--"];
    args.extend(paths.iter().map(String::as_str));
    let status = git_stdout_with_index(repo_root, &args, index_path)?;
    reject_rename_lines(status.lines())?;

    let mut unstaged_args = vec!["diff", "--name-status", "--find-renames", "--"];
    unstaged_args.extend(paths.iter().map(String::as_str));
    let unstaged = git_stdout_with_index(repo_root, &unstaged_args, index_path)?;
    reject_rename_lines(unstaged.lines())?;

    let mut staged_args = vec!["diff", "--cached", "--name-status", "--find-renames", "--"];
    staged_args.extend(paths.iter().map(String::as_str));
    let staged = git_stdout_with_index(repo_root, &staged_args, index_path)?;
    reject_rename_lines(staged.lines())?;

    Ok(())
}

fn reject_rename_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> anyhow::Result<()> {
    for line in lines {
        let status = line.split_whitespace().next().unwrap_or_default();
        let porcelain_code = line.get(0..2).unwrap_or_default();
        if status.starts_with('R') || porcelain_code.contains('R') || line.contains(" -> ") {
            anyhow::bail!(
                "stateful commit does not yet support rename path status for explicit paths"
            );
        }
    }
    Ok(())
}

fn restore_committed_paths_to_head(repo_root: &Path, paths: &[String]) -> anyhow::Result<()> {
    let mut args = vec!["restore", "--staged", "--source", "HEAD", "--"];
    args.extend(paths.iter().map(String::as_str));
    git_status(repo_root, &args)
}

fn git_status(repo_root: &Path, args: &[&str]) -> anyhow::Result<()> {
    git_status_with_index(repo_root, args, None)
}

fn git_status_with_index(
    repo_root: &Path,
    args: &[&str],
    index_path: Option<&Path>,
) -> anyhow::Result<()> {
    let mut command = git_command(repo_root, args, index_path);
    let output = command.output()?;

    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(())
}

struct TemporaryIndex {
    path: PathBuf,
}

impl TemporaryIndex {
    fn create(repo_root: &Path) -> anyhow::Result<Self> {
        let index_path = git_index_path(repo_root)?;
        let path = std::env::temp_dir().join(format!(
            "stateful-commit-index-{}-{}.index",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        if index_path.is_file() {
            fs::copy(&index_path, &path)?;
        }
        Ok(Self { path })
    }

    fn cleanup(&self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(self.path.with_extension("index.lock"));
    }
}

impl Drop for TemporaryIndex {
    fn drop(&mut self) {
        self.cleanup();
    }
}

struct TemporaryCommitMessage {
    path: PathBuf,
}

impl TemporaryCommitMessage {
    fn create(message: &str) -> anyhow::Result<Self> {
        let path = std::env::temp_dir().join(format!(
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

struct TemporaryHooksDir {
    path: PathBuf,
}

impl TemporaryHooksDir {
    fn create() -> anyhow::Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "stateful-empty-hooks-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&path)?;
        Ok(Self { path })
    }
}

impl Drop for TemporaryHooksDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.path);
    }
}

fn git_index_path(repo_root: &Path) -> anyhow::Result<PathBuf> {
    let path = git_stdout(repo_root, &["rev-parse", "--git-path", "index"])?;
    let path = PathBuf::from(path.trim());
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(repo_root.join(path))
    }
}

fn git_stdout(repo_root: &Path, args: &[&str]) -> anyhow::Result<String> {
    git_stdout_with_index(repo_root, args, None)
}

fn git_stdout_with_index(
    repo_root: &Path,
    args: &[&str],
    index_path: Option<&Path>,
) -> anyhow::Result<String> {
    Ok(String::from_utf8(git_stdout_bytes_with_index(
        repo_root, args, index_path,
    )?)?)
}

fn git_stdout_bytes_with_index(
    repo_root: &Path,
    args: &[&str],
    index_path: Option<&Path>,
) -> anyhow::Result<Vec<u8>> {
    let mut command = git_command(repo_root, args, index_path);
    let output = command.output()?;

    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(output.stdout)
}

fn git_command(repo_root: &Path, args: &[&str], index_path: Option<&Path>) -> Command {
    let mut command = Command::new("git");
    command.args(args).current_dir(repo_root);
    sanitize_git_environment(&mut command);
    if let Some(index_path) = index_path {
        command.env("GIT_INDEX_FILE", index_path);
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
    for (key, _value) in std::env::vars_os() {
        let key_name = key.to_string_lossy();
        if key_name.starts_with("GIT_CONFIG_") || key_name.starts_with("GIT_TRACE") {
            command.env_remove(&key);
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct CommitAuthorizeDecision {
    decision: String,
    message: String,
    #[serde(default)]
    required_next_action: Option<String>,
}
