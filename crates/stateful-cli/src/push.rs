use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub struct PushRequest {
    pub repo_root: PathBuf,
    pub remote: Option<String>,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PushResult {
    pub remote: String,
    pub branch: String,
}

pub fn run_structured_push(request: PushRequest) -> anyhow::Result<PushResult> {
    let PushRequest {
        repo_root,
        remote,
        branch,
    } = request;
    ensure_clean_worktree(&repo_root)?;
    let current_branch = current_branch(&repo_root)?;
    let target = resolve_push_target(&repo_root, &current_branch, remote, branch)?;
    validate_push_remote(&repo_root, &target.remote)?;
    validate_push_branch(&repo_root, &target.branch)?;

    let args = if target.explicit {
        vec!["push", target.remote.as_str(), target.branch.as_str()]
    } else {
        vec!["push"]
    };
    git_status(&repo_root, &args)?;

    Ok(PushResult {
        remote: target.remote,
        branch: target.branch,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PushTarget {
    remote: String,
    branch: String,
    explicit: bool,
}

fn resolve_push_target(
    repo_root: &Path,
    current_branch: &str,
    remote: Option<String>,
    branch: Option<String>,
) -> anyhow::Result<PushTarget> {
    match (remote, branch) {
        (None, None) => {
            ensure_upstream(repo_root)?;
            let remote = branch_remote(repo_root, current_branch)?;
            Ok(PushTarget {
                remote,
                branch: current_branch.to_string(),
                explicit: false,
            })
        }
        (Some(remote), Some(branch)) => {
            validate_push_value("remote", &remote)?;
            validate_push_value("branch", &branch)?;
            if branch != current_branch {
                anyhow::bail!(
                    "stateful push explicit branch `{branch}` must match current branch `{current_branch}`"
                );
            }
            Ok(PushTarget {
                remote,
                branch,
                explicit: true,
            })
        }
        _ => anyhow::bail!("stateful push requires both remote and branch, or neither"),
    }
}

fn ensure_clean_worktree(repo_root: &Path) -> anyhow::Result<()> {
    let status = git_stdout(
        repo_root,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )?;
    if !status.trim().is_empty() {
        anyhow::bail!(
            "stateful push requires a clean working tree; commit or remove local changes first"
        );
    }

    Ok(())
}

fn current_branch(repo_root: &Path) -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .current_dir(repo_root)
        .output()?;

    if !output.status.success() {
        anyhow::bail!("stateful push requires an attached branch");
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn ensure_upstream(repo_root: &Path) -> anyhow::Result<()> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .current_dir(repo_root)
        .output()?;

    if !output.status.success() {
        anyhow::bail!(
            "stateful push without an explicit target requires the current branch to have an upstream"
        );
    }

    Ok(())
}

fn branch_remote(repo_root: &Path, branch: &str) -> anyhow::Result<String> {
    let key = format!("branch.{branch}.remote");
    let remote = git_stdout(repo_root, &["config", "--get", &key])?;
    let remote = remote.trim();
    if remote.is_empty() {
        anyhow::bail!("stateful push could not resolve upstream remote for `{branch}`");
    }

    Ok(remote.to_string())
}

fn validate_push_remote(repo_root: &Path, remote: &str) -> anyhow::Result<()> {
    validate_push_value("remote", remote)?;
    git_status(repo_root, &["remote", "get-url", remote])
        .map_err(|_| anyhow::anyhow!("stateful push remote `{remote}` is not configured"))?;
    Ok(())
}

fn validate_push_branch(repo_root: &Path, branch: &str) -> anyhow::Result<()> {
    validate_push_value("branch", branch)?;
    if branch == "HEAD" {
        anyhow::bail!("stateful push branch must name the current branch, not HEAD");
    }

    git_status(repo_root, &["check-ref-format", "--branch", branch])
        .map_err(|_| anyhow::anyhow!("stateful push branch `{branch}` is not a valid branch name"))
}

fn validate_push_value(label: &str, value: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() {
        anyhow::bail!("push {label} is required");
    }
    if value.starts_with('-') {
        anyhow::bail!("push {label} must not start with '-'");
    }
    if value.contains(':') {
        anyhow::bail!("push {label} must not contain ':'");
    }
    if value
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control())
    {
        anyhow::bail!("push {label} must not contain whitespace or control characters");
    }

    Ok(())
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
