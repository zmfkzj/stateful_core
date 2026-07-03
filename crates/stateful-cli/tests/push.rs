use std::{fs, process::Command};

use stateful_cli::{PushRequest, run_structured_push};

#[test]
fn structured_push_pushes_current_branch_to_upstream() {
    let local = git_repo("stateful-push-upstream-local");
    let remote = bare_git_repo("stateful-push-upstream-remote");
    seed_commit(local.path(), "first");
    git(
        local.path(),
        &[
            "remote",
            "add",
            "origin",
            remote.path().to_str().expect("remote path should be utf8"),
        ],
    );
    git(local.path(), &["push", "--set-upstream", "origin", "main"]);
    seed_commit(local.path(), "second");

    let result = run_structured_push(PushRequest {
        repo_root: local.path().to_path_buf(),
        remote: None,
        branch: None,
    })
    .expect("push should succeed");

    let local_head = git_output(local.path(), &["rev-parse", "HEAD"]);
    let remote_head = git_output(remote.path(), &["rev-parse", "refs/heads/main"]);
    assert_eq!(result.remote, "origin");
    assert_eq!(result.branch, "main");
    assert_eq!(remote_head.trim(), local_head.trim());
}

#[test]
fn structured_push_requires_clean_worktree() {
    let local = git_repo("stateful-push-dirty-local");
    let remote = bare_git_repo("stateful-push-dirty-remote");
    seed_commit(local.path(), "first");
    git(
        local.path(),
        &[
            "remote",
            "add",
            "origin",
            remote.path().to_str().expect("remote path should be utf8"),
        ],
    );
    git(local.path(), &["push", "--set-upstream", "origin", "main"]);
    fs::write(local.path().join("README.md"), "dirty\n").expect("file should write");

    let result = run_structured_push(PushRequest {
        repo_root: local.path().to_path_buf(),
        remote: None,
        branch: None,
    });

    assert!(
        result
            .expect_err("dirty worktree should reject push")
            .to_string()
            .contains("clean working tree")
    );
}

#[test]
fn structured_push_rejects_force_like_targets() {
    let local = git_repo("stateful-push-force-like");

    let remote_result = run_structured_push(PushRequest {
        repo_root: local.path().to_path_buf(),
        remote: Some("--force".to_string()),
        branch: Some("main".to_string()),
    });
    assert!(
        remote_result
            .expect_err("force-like remote should be rejected")
            .to_string()
            .contains("push remote must not start with '-'")
    );

    let branch_result = run_structured_push(PushRequest {
        repo_root: local.path().to_path_buf(),
        remote: Some("origin".to_string()),
        branch: Some("--force".to_string()),
    });
    assert!(
        branch_result
            .expect_err("force-like branch should be rejected")
            .to_string()
            .contains("push branch must not start with '-'")
    );
}

fn git_repo(name: &str) -> tempfile_root::TempRoot {
    let root = tempfile_root::TempRoot::new(name);
    git(root.path(), &["init", "--initial-branch", "main"]);
    git(root.path(), &["config", "user.name", "stateful test"]);
    git(
        root.path(),
        &["config", "user.email", "stateful@example.invalid"],
    );
    root
}

fn bare_git_repo(name: &str) -> tempfile_root::TempRoot {
    let root = tempfile_root::TempRoot::new(name);
    git(root.path(), &["init", "--bare", "--initial-branch", "main"]);
    root
}

fn seed_commit(root: &std::path::Path, text: &str) {
    fs::write(root.join("README.md"), format!("{text}\n")).expect("file should write");
    git(root, &["add", "README.md"]);
    git(root, &["commit", "-m", &format!("docs: {text}")]);
}

fn git(root: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .expect("git should run");
    assert!(status.success(), "git {args:?} should succeed");
}

fn git_output(root: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git should run");
    assert!(output.status.success(), "git {args:?} should succeed");
    String::from_utf8(output.stdout).expect("git output should be utf8")
}

mod tempfile_root {
    use std::path::Path;

    pub struct TempRoot {
        root: tempfile::TempDir,
    }

    impl TempRoot {
        pub fn new(name: &str) -> Self {
            let root = tempfile::Builder::new()
                .prefix(&format!("{name}-"))
                .tempdir()
                .expect("temp dir should create");
            Self { root }
        }

        pub fn path(&self) -> &Path {
            self.root.path()
        }
    }
}
