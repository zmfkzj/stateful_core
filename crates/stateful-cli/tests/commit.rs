use std::{
    fs,
    process::Command,
    sync::{Arc, Mutex},
};

use stateful_cli::{CommitRequest, run_structured_commit};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

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
            .expect_err("empty commit message should be rejected")
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

    let absolute_plan = root
        .path()
        .join("docs/plan.md")
        .to_string_lossy()
        .to_string();
    let rejected_path_lists = vec![
        Vec::<String>::new(),
        vec![".".to_string()],
        vec!["*".to_string()],
        vec![":/".to_string()],
        vec!["-n".to_string()],
        vec!["docs/../plan.md".to_string()],
        vec!["docs/*.md".to_string()],
        vec![":(glob)docs/*.md".to_string()],
        vec!["docs//plan.md".to_string()],
        vec![absolute_plan],
        vec!["docs".to_string()],
        vec!["docs/".to_string()],
    ];

    for paths in rejected_path_lists {
        let result = run_structured_commit(CommitRequest {
            repo_root: root.path().to_path_buf(),
            message: "docs: add plan".to_string(),
            paths,
            session_id: Some("s1".to_string()),
            workspace_id: Some("w1".to_string()),
            authorize: Some(Box::new(|_action, path| {
                panic!("broad pathspec `{path}` should be rejected before authorization")
            })),
        });

        assert!(
            result
                .expect_err("broad pathspec should be rejected")
                .to_string()
                .contains("explicit file paths are required")
        );
        let staged = git_output(root.path(), &["diff", "--cached", "--name-only"]);
        assert!(staged.is_empty(), "broad pathspec should not mutate index");
    }
}

#[test]
fn structured_commit_normalizes_current_dir_file_paths() {
    let root = git_repo("stateful-commit-dot-slash-path");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "plan\n").expect("plan should write");

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: add plan".to_string(),
        paths: vec!["./docs/plan.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(|action, path| {
            assert_eq!(action, "write_file");
            assert_eq!(path, "docs/plan.md");
            Ok(())
        })),
    })
    .expect("dot-slash explicit file path should commit");

    assert_eq!(result.committed_paths, vec!["docs/plan.md"]);
}

#[test]
fn structured_commit_preserves_whitespace_in_explicit_file_paths() {
    let root = git_repo("stateful-commit-whitespace-path");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/report.md"), "normal\n").expect("normal report should write");
    fs::write(root.path().join("docs/report.md "), "spaced\n").expect("spaced report should write");

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: add spaced report".to_string(),
        paths: vec!["docs/report.md ".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(|action, path| {
            assert_eq!(action, "write_file");
            assert_eq!(path, "docs/report.md ");
            Ok(())
        })),
    })
    .expect("spaced explicit file path should commit");

    assert_eq!(result.committed_paths, vec!["docs/report.md "]);
    let tree = git_output(root.path(), &["ls-tree", "-rz", "--name-only", "HEAD"]);
    assert!(tree.split('\0').any(|line| line == "docs/report.md "));
    assert!(!tree.split('\0').any(|line| line == "docs/report.md"));
    assert_eq!(
        git_output(root.path(), &["show", "HEAD:docs/report.md "]),
        "spaced\n"
    );
}

#[test]
fn structured_commit_allows_explicit_tracked_file_under_ignored_directory() {
    let root = git_repo("stateful-commit-tracked-ignored-file");
    fs::create_dir_all(root.path().join("docs/superpowers/plans"))
        .expect("ignored docs dir should write");
    fs::write(
        root.path().join("docs/superpowers/plans/implementation.md"),
        "base\n",
    )
    .expect("tracked ignored file should write");
    git(
        root.path(),
        &["add", "docs/superpowers/plans/implementation.md"],
    );
    git(root.path(), &["commit", "-m", "docs: seed ignored plan"]);
    fs::write(root.path().join(".gitignore"), "docs/superpowers/\n")
        .expect("gitignore should write");
    fs::write(
        root.path().join("docs/superpowers/plans/implementation.md"),
        "updated\n",
    )
    .expect("tracked ignored file should update");

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: update ignored plan".to_string(),
        paths: vec!["docs/superpowers/plans/implementation.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(|action, path| {
            assert_eq!(action, "write_file");
            assert_eq!(path, "docs/superpowers/plans/implementation.md");
            Ok(())
        })),
    })
    .expect("explicit tracked file under ignored directory should commit");

    assert_eq!(
        result.committed_paths,
        vec!["docs/superpowers/plans/implementation.md"]
    );
    assert_eq!(
        git_output(
            root.path(),
            &["show", "HEAD:docs/superpowers/plans/implementation.md"]
        ),
        "updated\n"
    );
}

#[test]
fn structured_commit_rejects_deleted_tracked_directory_before_staging() {
    let root = git_repo("stateful-commit-deleted-directory");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "plan\n").expect("plan should write");
    fs::write(root.path().join("docs/other.md"), "other\n").expect("other should write");
    git(root.path(), &["add", "docs"]);
    git(root.path(), &["commit", "-m", "docs: seed"]);
    fs::remove_dir_all(root.path().join("docs")).expect("docs dir should be removable");

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: remove plan".to_string(),
        paths: vec!["docs".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(|_action, path| {
            panic!("directory pathspec `{path}` should be rejected before authorization")
        })),
    });

    assert!(
        result
            .expect_err("deleted directory path should be rejected")
            .to_string()
            .contains("explicit file paths are required")
    );
    let staged = git_output(root.path(), &["diff", "--cached", "--name-only"]);
    assert!(
        staged.is_empty(),
        "deleted tracked directory should not mutate index"
    );
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
            .expect_err("unrelated staged changes should be rejected")
            .to_string()
            .contains("unrelated staged changes")
    );
}

#[test]
fn structured_commit_authorizes_deleted_files_as_delete_file() {
    let root = git_repo("stateful-commit-delete-action");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "plan\n").expect("plan should write");
    git(root.path(), &["add", "docs/plan.md"]);
    git(root.path(), &["commit", "-m", "docs: seed"]);
    fs::remove_file(root.path().join("docs/plan.md")).expect("plan should remove");
    let authorized = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let authorized_for_closure = Arc::clone(&authorized);

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: remove plan".to_string(),
        paths: vec!["docs/plan.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(move |action, path| {
            authorized_for_closure
                .lock()
                .expect("authorization log should lock")
                .push((action.to_string(), path.to_string()));
            Ok(())
        })),
    })
    .expect("commit should succeed");

    assert_eq!(result.committed_paths, vec!["docs/plan.md"]);
    assert_eq!(
        *authorized.lock().expect("authorization log should lock"),
        vec![("delete_file".to_string(), "docs/plan.md".to_string())]
    );
    let show = git_output(root.path(), &["show", "--name-status", "--format=", "HEAD"]);
    assert!(show.lines().any(|line| line == "D\tdocs/plan.md"));
}

#[test]
fn structured_commit_does_not_allow_deleted_file_under_write_authorization() {
    let root = git_repo("stateful-commit-delete-policy");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "plan\n").expect("plan should write");
    git(root.path(), &["add", "docs/plan.md"]);
    git(root.path(), &["commit", "-m", "docs: seed"]);
    fs::remove_file(root.path().join("docs/plan.md")).expect("plan should remove");

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: remove plan".to_string(),
        paths: vec!["docs/plan.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(|action, path| {
            assert_eq!(path, "docs/plan.md");
            if action == "delete_file" {
                anyhow::bail!("delete requires exact file intent");
            }
            Ok(())
        })),
    });

    assert!(
        result
            .expect_err("denied delete authorization should fail")
            .to_string()
            .contains("delete requires exact file intent")
    );
    let staged = git_output(root.path(), &["diff", "--cached", "--name-only"]);
    assert!(staged.is_empty(), "denied delete should not mutate index");
}

#[test]
fn structured_commit_rejects_write_to_delete_race_after_authorization() {
    let root = git_repo("stateful-commit-write-delete-race");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "base\n").expect("plan should write");
    git(root.path(), &["add", "docs/plan.md"]);
    git(root.path(), &["commit", "-m", "docs: seed"]);
    fs::write(root.path().join("docs/plan.md"), "updated\n").expect("plan should update");
    let repo_path = root.path().to_path_buf();

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: update plan".to_string(),
        paths: vec!["docs/plan.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(move |action, path| {
            assert_eq!(action, "write_file");
            assert_eq!(path, "docs/plan.md");
            fs::remove_file(repo_path.join(path)).expect("plan should remove after authorization");
            Ok(())
        })),
    });

    let error = result.expect_err("write-to-delete race should fail");
    assert!(
        error
            .to_string()
            .contains("changed from write_file to delete_file")
    );
    let staged = git_output(root.path(), &["diff", "--cached", "--name-only"]);
    assert!(
        staged.is_empty(),
        "failed race should not mutate repo index"
    );
    assert_eq!(
        git_output(root.path(), &["rev-list", "--count", "HEAD"]),
        "1\n"
    );
}

#[test]
fn structured_commit_rejects_delete_to_write_race_after_authorization() {
    let root = git_repo("stateful-commit-delete-write-race");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "base\n").expect("plan should write");
    git(root.path(), &["add", "docs/plan.md"]);
    git(root.path(), &["commit", "-m", "docs: seed"]);
    fs::remove_file(root.path().join("docs/plan.md")).expect("plan should remove");
    let repo_path = root.path().to_path_buf();

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: restore plan".to_string(),
        paths: vec!["docs/plan.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(move |action, path| {
            assert_eq!(action, "delete_file");
            assert_eq!(path, "docs/plan.md");
            fs::write(repo_path.join(path), "restored\n")
                .expect("plan should be restored after authorization");
            Ok(())
        })),
    });

    let error = result.expect_err("delete-to-write race should fail");
    assert!(
        error
            .to_string()
            .contains("changed from delete_file to write_file")
    );
    let staged = git_output(root.path(), &["diff", "--cached", "--name-only"]);
    assert!(
        staged.is_empty(),
        "failed race should not mutate repo index"
    );
    assert_eq!(
        git_output(root.path(), &["rev-list", "--count", "HEAD"]),
        "1\n"
    );
}

#[cfg(unix)]
#[test]
fn structured_commit_rejects_pre_commit_hook_write_to_delete_action_change() {
    let root = git_repo("stateful-commit-hook-write-delete-race");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "base\n").expect("plan should write");
    git(root.path(), &["add", "docs/plan.md"]);
    git(root.path(), &["commit", "-m", "docs: seed"]);
    fs::write(root.path().join("docs/plan.md"), "updated\n").expect("plan should update");
    let hook_path = root.path().join(".git/hooks/pre-commit");
    fs::write(
        &hook_path,
        "#!/bin/sh\nrm docs/plan.md\ngit add docs/plan.md\nexit 0\n",
    )
    .expect("pre-commit hook should write");
    let mut permissions = fs::metadata(&hook_path)
        .expect("pre-commit metadata should load")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook_path, permissions).expect("pre-commit hook should be executable");

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: update plan".to_string(),
        paths: vec!["docs/plan.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(|action, path| {
            assert_eq!(action, "write_file");
            assert_eq!(path, "docs/plan.md");
            Ok(())
        })),
    });

    let error = result.expect_err("hook write-to-delete action change should fail");
    assert!(
        error
            .to_string()
            .contains("changed from write_file to delete_file")
    );
    let staged = git_output(root.path(), &["diff", "--cached", "--name-only"]);
    assert!(
        staged.is_empty(),
        "failed hook race should not mutate repo index"
    );
    assert_eq!(
        git_output(root.path(), &["rev-list", "--count", "HEAD"]),
        "1\n"
    );
}

#[cfg(unix)]
#[test]
fn structured_commit_rejects_pre_commit_hook_delete_to_write_action_change() {
    let root = git_repo("stateful-commit-hook-delete-write-race");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "base\n").expect("plan should write");
    git(root.path(), &["add", "docs/plan.md"]);
    git(root.path(), &["commit", "-m", "docs: seed"]);
    fs::remove_file(root.path().join("docs/plan.md")).expect("plan should remove");
    let hook_path = root.path().join(".git/hooks/pre-commit");
    fs::write(
        &hook_path,
        "#!/bin/sh\nprintf 'restored\\n' > docs/plan.md\ngit add docs/plan.md\nexit 0\n",
    )
    .expect("pre-commit hook should write");
    let mut permissions = fs::metadata(&hook_path)
        .expect("pre-commit metadata should load")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook_path, permissions).expect("pre-commit hook should be executable");

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: restore plan".to_string(),
        paths: vec!["docs/plan.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(|action, path| {
            assert_eq!(action, "delete_file");
            assert_eq!(path, "docs/plan.md");
            Ok(())
        })),
    });

    let error = result.expect_err("hook delete-to-write action change should fail");
    assert!(
        error
            .to_string()
            .contains("changed from delete_file to write_file")
    );
    let staged = git_output(root.path(), &["diff", "--cached", "--name-only"]);
    assert!(
        staged.is_empty(),
        "failed hook race should not mutate repo index"
    );
    assert_eq!(
        git_output(root.path(), &["rev-list", "--count", "HEAD"]),
        "1\n"
    );
}

#[cfg(unix)]
#[test]
fn structured_commit_uses_validated_index_when_hook_changes_worktree_without_staging() {
    let root = git_repo("stateful-commit-hook-worktree-delete");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "base\n").expect("plan should write");
    git(root.path(), &["add", "docs/plan.md"]);
    git(root.path(), &["commit", "-m", "docs: seed"]);
    fs::write(root.path().join("docs/plan.md"), "updated\n").expect("plan should update");
    let hook_path = root.path().join(".git/hooks/pre-commit");
    fs::write(&hook_path, "#!/bin/sh\nrm docs/plan.md\nexit 0\n")
        .expect("pre-commit hook should write");
    let mut permissions = fs::metadata(&hook_path)
        .expect("pre-commit metadata should load")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook_path, permissions).expect("pre-commit hook should be executable");

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: update plan".to_string(),
        paths: vec!["docs/plan.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(|action, path| {
            assert_eq!(action, "write_file");
            assert_eq!(path, "docs/plan.md");
            Ok(())
        })),
    })
    .expect("commit should use validated temporary index");

    assert_eq!(result.committed_paths, vec!["docs/plan.md"]);
    assert_eq!(
        git_output(root.path(), &["show", "HEAD:docs/plan.md"]),
        "updated\n"
    );
}

#[cfg(unix)]
#[test]
fn structured_commit_provides_git_locator_env_to_hooks() {
    let root = git_repo("stateful-commit-hook-git-env");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "plan\n").expect("plan should write");
    let hook_path = root.path().join(".git/hooks/pre-commit");
    fs::write(
        &hook_path,
        "#!/bin/sh\ntest -n \"$GIT_DIR\" || { echo missing GIT_DIR >&2; exit 1; }\ntest -n \"$GIT_WORK_TREE\" || { echo missing GIT_WORK_TREE >&2; exit 1; }\ngit --git-dir=\"$GIT_DIR\" rev-parse --git-dir >/dev/null\n",
    )
    .expect("pre-commit hook should write");
    let mut permissions = fs::metadata(&hook_path)
        .expect("pre-commit metadata should load")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook_path, permissions).expect("pre-commit hook should be executable");

    run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: add plan".to_string(),
        paths: vec!["docs/plan.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(|action, path| {
            assert_eq!(action, "write_file");
            assert_eq!(path, "docs/plan.md");
            Ok(())
        })),
    })
    .expect("commit should provide Git hook locator env");
}

#[cfg(unix)]
#[test]
fn structured_commit_rejects_prepare_commit_msg_hook_action_change() {
    let root = git_repo("stateful-commit-prepare-hook-action-race");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "base\n").expect("plan should write");
    git(root.path(), &["add", "docs/plan.md"]);
    git(root.path(), &["commit", "-m", "docs: seed"]);
    fs::write(root.path().join("docs/plan.md"), "updated\n").expect("plan should update");
    let hook_path = root.path().join(".git/hooks/prepare-commit-msg");
    fs::write(
        &hook_path,
        "#!/bin/sh\nrm docs/plan.md\ngit add docs/plan.md\nexit 0\n",
    )
    .expect("prepare-commit-msg hook should write");
    let mut permissions = fs::metadata(&hook_path)
        .expect("prepare-commit-msg metadata should load")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook_path, permissions).expect("prepare-commit-msg should be executable");

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: update plan".to_string(),
        paths: vec!["docs/plan.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(|action, path| {
            assert_eq!(action, "write_file");
            assert_eq!(path, "docs/plan.md");
            Ok(())
        })),
    });

    let error = result.expect_err("prepare hook action change should fail");
    assert!(
        error
            .to_string()
            .contains("changed from write_file to delete_file")
    );
    let staged = git_output(root.path(), &["diff", "--cached", "--name-only"]);
    assert!(
        staged.is_empty(),
        "failed prepare hook race should not mutate repo index"
    );
    assert_eq!(
        git_output(root.path(), &["rev-list", "--count", "HEAD"]),
        "1\n"
    );
}

#[cfg(unix)]
#[test]
fn structured_commit_runs_commit_msg_hook_before_committing() {
    let root = git_repo("stateful-commit-msg-hook-policy");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("README.md"), "seed\n").expect("readme should write");
    git(root.path(), &["add", "README.md"]);
    git(root.path(), &["commit", "-m", "docs: seed"]);
    fs::write(root.path().join("docs/plan.md"), "plan\n").expect("plan should write");
    let hook_path = root.path().join(".git/hooks/commit-msg");
    fs::write(&hook_path, "#!/bin/sh\ngrep -q '^JIRA-' \"$1\" || exit 1\n")
        .expect("commit-msg hook should write");
    let mut permissions = fs::metadata(&hook_path)
        .expect("commit-msg metadata should load")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook_path, permissions).expect("commit-msg hook should be executable");

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: add plan".to_string(),
        paths: vec!["docs/plan.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(|action, path| {
            assert_eq!(action, "write_file");
            assert_eq!(path, "docs/plan.md");
            Ok(())
        })),
    });

    assert!(
        result
            .expect_err("commit-msg hook should reject message")
            .to_string()
            .contains("commit-msg hook failed")
    );
    assert_eq!(
        git_output(root.path(), &["rev-list", "--count", "HEAD"]),
        "1\n"
    );
}

#[cfg(unix)]
#[test]
fn structured_commit_runs_post_commit_hook_after_successful_commit() {
    let root = git_repo("stateful-post-commit-hook");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "plan\n").expect("plan should write");
    let hook_path = root.path().join(".git/hooks/post-commit");
    fs::write(&hook_path, "#!/bin/sh\nprintf ran > .post-commit-ran\n")
        .expect("post-commit hook should write");
    let mut permissions = fs::metadata(&hook_path)
        .expect("post-commit metadata should load")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook_path, permissions).expect("post-commit hook should be executable");

    run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: add plan".to_string(),
        paths: vec!["docs/plan.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(|action, path| {
            assert_eq!(action, "write_file");
            assert_eq!(path, "docs/plan.md");
            Ok(())
        })),
    })
    .expect("commit should succeed");

    assert_eq!(
        fs::read_to_string(root.path().join(".post-commit-ran"))
            .expect("post-commit marker should read"),
        "ran"
    );
}

#[test]
fn structured_commit_rejects_unstaged_rename_across_explicit_paths() {
    let root = git_repo("stateful-commit-unstaged-rename");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/old.md"), "plan\n").expect("old file should write");
    git(root.path(), &["add", "docs/old.md"]);
    git(root.path(), &["commit", "-m", "docs: seed"]);
    fs::rename(
        root.path().join("docs/old.md"),
        root.path().join("docs/new.md"),
    )
    .expect("file should rename");

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: rename plan".to_string(),
        paths: vec!["docs/old.md".to_string(), "docs/new.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(|_action, _path| Ok(()))),
    });

    assert!(
        result
            .expect_err("unstaged rename should be rejected")
            .to_string()
            .contains("rename path status")
    );
    let staged = git_output(root.path(), &["diff", "--cached", "--name-only"]);
    assert!(staged.is_empty(), "rejected rename should not mutate index");
}

#[test]
fn structured_commit_rejects_staged_git_mv_across_explicit_paths() {
    let root = git_repo("stateful-commit-staged-rename");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/old.md"), "plan\n").expect("old file should write");
    git(root.path(), &["add", "docs/old.md"]);
    git(root.path(), &["commit", "-m", "docs: seed"]);
    git(root.path(), &["mv", "docs/old.md", "docs/new.md"]);

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: rename plan".to_string(),
        paths: vec!["docs/old.md".to_string(), "docs/new.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(|_action, _path| Ok(()))),
    });

    assert!(
        result
            .expect_err("staged rename should be rejected")
            .to_string()
            .contains("rename path status")
    );
}

#[test]
fn structured_commit_allows_copied_file_status_as_new_file() {
    let root = git_repo("stateful-commit-copy-status");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/template.md"), "copied content\n")
        .expect("template should write");
    git(root.path(), &["add", "docs/template.md"]);
    git(root.path(), &["commit", "-m", "docs: seed"]);
    fs::copy(
        root.path().join("docs/template.md"),
        root.path().join("docs/copy.md"),
    )
    .expect("file should copy");

    let authorized = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let authorized_for_commit = Arc::clone(&authorized);
    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: copy template".to_string(),
        paths: vec!["docs/copy.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(move |action, path| {
            authorized_for_commit
                .lock()
                .expect("authorized actions should lock")
                .push((action.to_string(), path.to_string()));
            Ok(())
        })),
    })
    .expect("copied file should commit as a new file");

    assert_eq!(result.committed_paths, vec!["docs/copy.md"]);
    assert_eq!(
        *authorized.lock().expect("authorized actions should lock"),
        vec![("write_file".to_string(), "docs/copy.md".to_string())]
    );
    let last_commit = git_output(root.path(), &["show", "--name-only", "--format=", "HEAD"]);
    assert!(last_commit.lines().any(|line| line == "docs/copy.md"));
}

#[test]
fn structured_commit_allows_independent_delete_and_add_across_explicit_paths() {
    let root = git_repo("stateful-commit-delete-and-add");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/old.md"), "short old file\n").expect("old file should write");
    git(root.path(), &["add", "docs/old.md"]);
    git(root.path(), &["commit", "-m", "docs: seed"]);
    fs::remove_file(root.path().join("docs/old.md")).expect("old file should remove");
    fs::write(
        root.path().join("docs/new.md"),
        "new file with unrelated content\nand another line\n",
    )
    .expect("new file should write");
    let authorized = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let authorized_for_closure = Arc::clone(&authorized);

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: replace plan files".to_string(),
        paths: vec!["docs/old.md".to_string(), "docs/new.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(move |action, path| {
            authorized_for_closure
                .lock()
                .expect("authorization log should lock")
                .push((action.to_string(), path.to_string()));
            Ok(())
        })),
    })
    .expect("independent delete and add should commit");

    assert_eq!(
        result.committed_paths,
        vec!["docs/new.md".to_string(), "docs/old.md".to_string()]
    );
    assert_eq!(
        *authorized.lock().expect("authorization log should lock"),
        vec![
            ("write_file".to_string(), "docs/new.md".to_string()),
            ("delete_file".to_string(), "docs/old.md".to_string())
        ]
    );
    let show = git_output(root.path(), &["show", "--name-status", "--format=", "HEAD"]);
    assert!(show.lines().any(|line| line == "A\tdocs/new.md"));
    assert!(show.lines().any(|line| line == "D\tdocs/old.md"));
}

#[test]
fn structured_commit_stages_only_explicit_paths_and_commits() {
    let root = git_repo("stateful-commit-success");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "plan\n").expect("plan should write");
    fs::write(root.path().join("docs/untracked.md"), "untracked\n")
        .expect("untracked should write");

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: add plan".to_string(),
        paths: vec!["docs/plan.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(|action, path| {
            assert_eq!(action, "write_file");
            assert_eq!(path, "docs/plan.md");
            Ok(())
        })),
    })
    .expect("commit should succeed");

    assert_eq!(result.committed_paths, vec!["docs/plan.md"]);
    let head = git_output(root.path(), &["rev-parse", "HEAD"]);
    assert_eq!(result.commit_sha, head.trim());
    let show = git_output(root.path(), &["show", "--name-only", "--format=", "HEAD"]);
    assert!(show.lines().any(|line| line == "docs/plan.md"));
    assert!(!show.lines().any(|line| line == "docs/untracked.md"));
}

#[cfg(unix)]
#[test]
fn structured_commit_restores_original_index_after_commit_hook_failure() {
    let root = git_repo("stateful-commit-hook-failure");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "base\n").expect("plan should write");
    git(root.path(), &["add", "docs/plan.md"]);
    git(root.path(), &["commit", "-m", "docs: seed"]);
    fs::write(root.path().join("docs/plan.md"), "staged\n").expect("staged plan should write");
    git(root.path(), &["add", "docs/plan.md"]);
    fs::write(root.path().join("docs/plan.md"), "worktree\n").expect("worktree plan should write");
    let hook_path = root.path().join(".git/hooks/pre-commit");
    fs::write(&hook_path, "#!/bin/sh\nexit 1\n").expect("pre-commit hook should write");
    let mut permissions = fs::metadata(&hook_path)
        .expect("pre-commit metadata should load")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook_path, permissions).expect("pre-commit hook should be executable");

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: update plan".to_string(),
        paths: vec!["docs/plan.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(|action, path| {
            assert_eq!(action, "write_file");
            assert_eq!(path, "docs/plan.md");
            Ok(())
        })),
    });

    assert!(
        result
            .expect_err("failing pre-commit hook should fail commit")
            .to_string()
            .contains("pre-commit hook failed")
    );
    let staged = git_output(root.path(), &["diff", "--cached", "--", "docs/plan.md"]);
    assert!(staged.contains("+staged"));
    assert!(!staged.contains("+worktree"));
    let worktree = fs::read_to_string(root.path().join("docs/plan.md")).expect("plan should read");
    assert_eq!(worktree, "worktree\n");
}

#[cfg(unix)]
#[test]
fn structured_commit_clears_index_after_initial_commit_hook_failure() {
    let root = git_repo("stateful-commit-initial-hook-failure");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "plan\n").expect("plan should write");
    let hook_path = root.path().join(".git/hooks/pre-commit");
    fs::write(&hook_path, "#!/bin/sh\nexit 1\n").expect("pre-commit hook should write");
    let mut permissions = fs::metadata(&hook_path)
        .expect("pre-commit metadata should load")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook_path, permissions).expect("pre-commit hook should be executable");

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: initial plan".to_string(),
        paths: vec!["docs/plan.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(|action, path| {
            assert_eq!(action, "write_file");
            assert_eq!(path, "docs/plan.md");
            Ok(())
        })),
    });

    assert!(
        result
            .expect_err("failing pre-commit hook should fail initial commit")
            .to_string()
            .contains("pre-commit hook failed")
    );
    let staged = git_output(root.path(), &["diff", "--cached", "--name-only"]);
    assert!(
        staged.is_empty(),
        "failed initial commit should not leave index entries"
    );
    let worktree = fs::read_to_string(root.path().join("docs/plan.md")).expect("plan should read");
    assert_eq!(worktree, "plan\n");
}

#[cfg(unix)]
#[test]
fn structured_commit_rejects_unrelated_index_entries_added_by_successful_hook() {
    let root = git_repo("stateful-commit-hook-success-adds-unrelated");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "base\n").expect("plan should write");
    git(root.path(), &["add", "docs/plan.md"]);
    git(root.path(), &["commit", "-m", "docs: seed"]);
    fs::write(root.path().join("docs/plan.md"), "worktree\n").expect("plan should write");
    let hook_path = root.path().join(".git/hooks/pre-commit");
    fs::write(
        &hook_path,
        "#!/bin/sh\nprintf 'generated\\n' > generated.txt\ngit add generated.txt\nexit 0\n",
    )
    .expect("pre-commit hook should write");
    let mut permissions = fs::metadata(&hook_path)
        .expect("pre-commit metadata should load")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook_path, permissions).expect("pre-commit hook should be executable");

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: update plan".to_string(),
        paths: vec!["docs/plan.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(|action, path| {
            assert_eq!(action, "write_file");
            assert_eq!(path, "docs/plan.md");
            Ok(())
        })),
    });

    assert!(
        result
            .expect_err("hook staging unrelated file should fail commit")
            .to_string()
            .contains("unrelated staged changes")
    );
    assert_eq!(
        git_output(root.path(), &["rev-list", "--count", "HEAD"]),
        "1\n"
    );

    let staged = git_output(root.path(), &["diff", "--cached", "--name-only"]);
    assert!(
        staged.is_empty(),
        "rejected hook should not leave unrelated staged files"
    );
    let status = git_output(root.path(), &["status", "--short", "--", "generated.txt"]);
    assert!(
        status.lines().any(|line| line == "?? generated.txt"),
        "generated hook output should remain as an untracked worktree file"
    );
}

#[cfg(unix)]
#[test]
fn structured_commit_restores_unrelated_index_entries_added_by_failed_hook() {
    let root = git_repo("stateful-commit-hook-adds-unrelated");
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "base\n").expect("plan should write");
    git(root.path(), &["add", "docs/plan.md"]);
    git(root.path(), &["commit", "-m", "docs: seed"]);
    fs::write(root.path().join("docs/plan.md"), "worktree\n").expect("plan should write");
    let hook_path = root.path().join(".git/hooks/pre-commit");
    fs::write(
        &hook_path,
        "#!/bin/sh\nprintf 'generated\\n' > generated.txt\ngit add generated.txt\nexit 1\n",
    )
    .expect("pre-commit hook should write");
    let mut permissions = fs::metadata(&hook_path)
        .expect("pre-commit metadata should load")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook_path, permissions).expect("pre-commit hook should be executable");

    let result = run_structured_commit(CommitRequest {
        repo_root: root.path().to_path_buf(),
        message: "docs: update plan".to_string(),
        paths: vec!["docs/plan.md".to_string()],
        session_id: Some("s1".to_string()),
        workspace_id: Some("w1".to_string()),
        authorize: Some(Box::new(|action, path| {
            assert_eq!(action, "write_file");
            assert_eq!(path, "docs/plan.md");
            Ok(())
        })),
    });

    assert!(
        result
            .expect_err("failing pre-commit hook should fail commit")
            .to_string()
            .contains("pre-commit hook failed")
    );
    let staged = git_output(root.path(), &["diff", "--cached", "--name-only"]);
    assert!(
        staged.is_empty(),
        "failed hook should not leave unrelated staged files"
    );
    assert!(root.path().join("generated.txt").exists());
}

fn git_repo(name: &str) -> tempfile_root::TempRoot {
    let root = tempfile_root::TempRoot::new(name);
    git(root.path(), &["init"]);
    git(root.path(), &["config", "user.name", "stateful test"]);
    git(
        root.path(),
        &["config", "user.email", "stateful@example.invalid"],
    );
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
