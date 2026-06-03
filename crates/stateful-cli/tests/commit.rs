use std::{fs, process::Command};

use stateful_cli::{CommitRequest, run_structured_commit};

#[test]
fn structured_commit_rejects_empty_message() {
    let root = git_repo("stateful-commit-empty-message");
    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: " ".to_string(),
        paths: vec!["docs/plan.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: None,
    });

    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("commit message is required")
    );
}

#[test]
fn structured_commit_rejects_broad_pathspecs() {
    let root = git_repo("stateful-commit-broad-path");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "plan\n").expect("plan should write");
    fs::write(root.path().join("docs/other.md"), "other\n").expect("other should write");

    let rejected_path_lists = [
        Vec::<String>::new(),
        vec![".".to_string()],
        vec!["*".to_string()],
        vec![":/".to_string()],
        vec!["-n".to_string()],
        vec!["docs/../plan.md".to_string()],
        vec!["docs/*.md".to_string()],
        vec![":(glob)docs/*.md".to_string()],
    ];

    for paths in rejected_path_lists {
        let result = run_structured_commit(CommitRequest {
            repo_root: root.path().to_path_buf(),
            message: "docs: add plan".to_string(),
            paths,
            session_id: Some("s1".to_string()),
            workspace_id: Some("w1".to_string()),
            authorize: Some(Box::new(|path| {
                panic!("broad pathspec `{path}` should be rejected before authorization")
            })),
        });

        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("explicit file paths are required")
        );
        let staged = git_output(root.path(), &["diff", "--cached", "--name-only"]);
        assert!(staged.is_empty(), "broad pathspec should not mutate index");
    }
}

#[test]
fn structured_commit_rejects_unrelated_staged_changes() {
    let root = git_repo("stateful-commit-unrelated-staged");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "plan\n").expect("plan should write");
    fs::write(root.path().join("docs/other.md"), "other\n").expect("other should write");
    git(root.path(), &["add", "docs/other.md"]);

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: add plan".to_string(),
        paths: vec!["docs/plan.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: None,
    });

    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("unrelated staged changes")
    );
}

#[test]
fn structured_commit_stages_only_explicit_paths_and_commits() {
    let root = git_repo("stateful-commit-success");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "plan\n").expect("plan should write");
    fs::write(root.path().join("docs/untracked.md"), "untracked\n").expect("untracked should write");

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: add plan".to_string(),
        paths: vec!["docs/plan.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(|path| {
            assert_eq!(path, "docs/plan.md");
            Ok(())
        })),
    })
    .expect("commit should succeed");

    assert_eq!(result.committed_paths, vec!["docs/plan.md"]);
    let show = git_output(root.path(), &["show", "--name-only", "--format=", "HEAD"]);
    assert!(show.lines().any(|line| line == "docs/plan.md"));
    assert!(!show.lines().any(|line| line == "docs/untracked.md"));
}

fn git_repo(name: &str) -> tempfile_root::TempRoot {
    let root = tempfile_root::TempRoot::new(name);
    git(root.path(), &["init"]);
    git(root.path(), &["config", "user.name", "stateful test"]);
    git(root.path(), &["config", "user.email", "stateful@example.invalid"]);
    root
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
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    pub struct TempRoot {
        path: PathBuf,
    }

    impl TempRoot {
        pub fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
            if path.exists() {
                fs::remove_dir_all(&path).expect("old temp root should be removable");
            }
            fs::create_dir_all(&path).expect("temp root should be creatable");
            Self { path }
        }

        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
