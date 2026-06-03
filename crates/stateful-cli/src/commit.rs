use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{discover_runtime_with_optional_global, post_json};

pub type AuthorizePath = Box<dyn Fn(&str, &str) -> anyhow::Result<()> + Send + Sync>;

pub struct CommitRequest {
    pub repo_root: PathBuf,
    pub message: String,
    pub paths: Vec<String>,
    pub session_id: Option<String>,
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
    reject_rename_or_copy_status(&request.repo_root, &paths)?;
    let targets = paths
        .iter()
        .map(|path| commit_target(&request.repo_root, path))
        .collect::<anyhow::Result<Vec<_>>>()?;

    for target in &targets {
        authorize_path(&request, target)?;
    }

    let mut add_args = vec!["add", "--"];
    add_args.extend(paths.iter().map(String::as_str));
    git_status(&request.repo_root, &add_args)?;

    deny_unrelated_staged_changes(&request.repo_root, &explicit)?;

    let mut commit_args = vec!["commit", "-m", message, "--"];
    commit_args.extend(paths.iter().map(String::as_str));
    git_status(&request.repo_root, &commit_args)?;

    let commit_sha = git_stdout(&request.repo_root, &["rev-parse", "HEAD"])?;
    Ok(CommitResult {
        commit_sha: commit_sha.trim().to_string(),
        committed_paths: paths,
    })
}

fn normalize_explicit_paths(repo_root: &Path, paths: &[String]) -> anyhow::Result<Vec<String>> {
    if paths.is_empty() {
        anyhow::bail!("explicit file paths are required");
    }

    let mut normalized = Vec::new();
    for path in paths {
        let path = path.trim();
        if is_broad_pathspec(repo_root, path) {
            anyhow::bail!("explicit file paths are required; rejected pathspec `{path}`");
        }
        let normalized_path = path.replace('\\', "/");
        if matches_tracked_directory(repo_root, &normalized_path)? {
            anyhow::bail!("explicit file paths are required; rejected pathspec `{path}`");
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
        || path.starts_with("./")
        || path.contains("..")
        || path.contains("//")
        || path.contains("/./")
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
    let staged = git_stdout(repo_root, &["diff", "--cached", "--name-only"])?;
    let unrelated = staged
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.replace('\\', "/"))
        .filter(|path| !explicit.contains(path))
        .collect::<Vec<_>>();

    if !unrelated.is_empty() {
        anyhow::bail!(
            "unrelated staged changes are present: {}",
            unrelated.join(", ")
        );
    }

    Ok(())
}

fn authorize_path(request: &CommitRequest, target: &CommitTarget) -> anyhow::Result<()> {
    if let Some(authorize) = &request.authorize {
        return authorize(target.action, &target.path);
    }

    let session_id = request
        .session_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("stateful commit requires a current session id"))?;
    let runtime = discover_runtime_with_optional_global(&request.repo_root)?;
    let workspace_id = request
        .workspace_id
        .as_deref()
        .unwrap_or(runtime.workspace_id.as_str());
    let response = post_json(
        &runtime,
        "/v1/authorize",
        &serde_json::json!({
            "session_id": session_id,
            "workspace_id": workspace_id,
            "action": target.action,
            "path": target.path,
        }),
    )?;

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

fn is_missing_tracked_file(repo_root: &Path, path: &str) -> anyhow::Result<bool> {
    if repo_root.join(path).exists() {
        return Ok(false);
    }

    Ok(Command::new("git")
        .args(["ls-files", "--error-unmatch", "--", path])
        .current_dir(repo_root)
        .output()?
        .status
        .success())
}

fn reject_rename_or_copy_status(repo_root: &Path, paths: &[String]) -> anyhow::Result<()> {
    let mut args = vec!["status", "--porcelain=v1", "--find-renames", "--"];
    args.extend(paths.iter().map(String::as_str));
    let status = git_stdout(repo_root, &args)?;
    for line in status.lines() {
        let code = line.get(0..2).unwrap_or_default();
        if code.contains('R') || code.contains('C') || line.contains(" -> ") {
            anyhow::bail!(
                "stateful commit does not yet support rename/copy path status for explicit paths"
            );
        }
    }

    let has_missing_tracked = paths
        .iter()
        .map(|path| is_missing_tracked_file(repo_root, path))
        .collect::<anyhow::Result<Vec<_>>>()?
        .into_iter()
        .any(|missing| missing);
    let has_new_file = paths
        .iter()
        .map(|path| path_is_new_in_worktree_or_index(repo_root, path))
        .collect::<anyhow::Result<Vec<_>>>()?
        .into_iter()
        .any(|is_new| is_new);
    if has_missing_tracked && has_new_file {
        anyhow::bail!(
            "stateful commit does not yet support rename/copy path status for explicit paths"
        );
    }

    Ok(())
}

fn path_is_new_in_worktree_or_index(repo_root: &Path, path: &str) -> anyhow::Result<bool> {
    if !repo_root.join(path).exists() {
        return Ok(false);
    }

    Ok(!Command::new("git")
        .args(["cat-file", "-e", &format!("HEAD:{path}")])
        .current_dir(repo_root)
        .output()?
        .status
        .success())
}

fn git_status(repo_root: &Path, args: &[&str]) -> anyhow::Result<()> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()?;

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
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()?;

    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8(output.stdout)?)
}

#[derive(Debug, serde::Deserialize)]
struct CommitAuthorizeDecision {
    decision: String,
    message: String,
    #[serde(default)]
    required_next_action: Option<String>,
}
