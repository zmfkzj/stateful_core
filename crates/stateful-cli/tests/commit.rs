use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    sync::{Arc, Mutex, mpsc},
    thread,
};

use stateful_cli::{
    CommitRequest, CurrentSession, GlobalPaths, ServerRuntime, run_structured_commit,
    write_current_session_file, write_global_runtime_file,
};

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
        vec!["./docs/plan.md".to_string()],
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
                .unwrap_err()
                .to_string()
                .contains("explicit file paths are required")
        );
        let staged = git_output(root.path(), &["diff", "--cached", "--name-only"]);
        assert!(staged.is_empty(), "broad pathspec should not mutate index");
    }
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
            .unwrap_err()
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
            .unwrap_err()
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
            .unwrap_err()
            .to_string()
            .contains("delete requires exact file intent")
    );
    let staged = git_output(root.path(), &["diff", "--cached", "--name-only"]);
    assert!(staged.is_empty(), "denied delete should not mutate index");
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
        authorize: Some(Box::new(|action, path| {
            panic!("rename target `{path}` with `{action}` should be rejected before authorization")
        })),
    });

    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("rename/copy path status")
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
        authorize: Some(Box::new(|action, path| {
            panic!(
                "staged rename target `{path}` with `{action}` should be rejected before authorization"
            )
        })),
    });

    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("rename/copy path status")
    );
}

#[test]
fn structured_commit_command_from_subdir_uses_git_root_session_and_paths() {
    let root = git_repo("stateful-commit-subdir");
    let paths = GlobalPaths::new(root.path().join("home"));
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "plan\n").expect("plan should write");
    write_current_session_file(root.path(), &CurrentSession::new("s-subdir", "w-session"))
        .expect("current session should write");
    let (runtime, rx) = spawn_fake_authorize_server();
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .args(["commit", "-m", "docs: add plan", "--", "plan.md"])
        .current_dir(root.path().join("docs"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("STATEFUL_HOME", &paths.home)
        .output()
        .expect("stateful commit should run");

    assert!(
        output.status.success(),
        "stateful commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("\"session_id\":\"s-subdir\""));
    assert!(request.contains("\"workspace_id\":\"w-session\""));
    assert!(request.contains("\"path\":\"docs/plan.md\""));
    let show = git_output(root.path(), &["show", "--name-only", "--format=", "HEAD"]);
    assert!(show.lines().any(|line| line == "docs/plan.md"));
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

#[test]
fn structured_commit_command_discovers_global_runtime_for_authorization() {
    let root = git_repo("stateful-commit-global-runtime");
    let paths = GlobalPaths::new(root.path().join("home"));
    fs::create_dir_all(root.path().join("docs")).expect("docs dir should write");
    fs::write(root.path().join("docs/plan.md"), "plan\n").expect("plan should write");
    write_current_session_file(root.path(), &CurrentSession::new("s-global", "w-session"))
        .expect("current session should write");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_json_response(
            &mut stream,
            r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
        );
    });
    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "global-w", 42);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let output = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .args(["commit", "-m", "docs: add plan", "--", "docs/plan.md"])
        .current_dir(root.path())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("STATEFUL_HOME", &paths.home)
        .output()
        .expect("stateful commit should run");

    assert!(
        output.status.success(),
        "stateful commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/authorize HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));
    assert!(request.contains("\"session_id\":\"s-global\""));
    assert!(request.contains("\"workspace_id\":\"w-session\""));
    assert!(request.contains("\"path\":\"docs/plan.md\""));
}

fn spawn_fake_authorize_server() -> (ServerRuntime, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        write_json_response(
            &mut stream,
            r#"{"decision":"allow","reason_code":"authorized","message":"ok","required_next_action":null}"#,
        );
    });
    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "global-w", 42);
    (runtime, rx)
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

fn read_http_request(stream: &mut std::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut byte = [0_u8; 1];
    while !buffer.ends_with(b"\r\n\r\n") {
        stream
            .read_exact(&mut byte)
            .expect("request header byte should read");
        buffer.push(byte[0]);
    }

    let headers = String::from_utf8(buffer.clone()).expect("headers should be utf8");
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length: "))
        .expect("content length should exist")
        .parse::<usize>()
        .expect("content length should parse");

    let mut body = vec![0_u8; content_length];
    stream
        .read_exact(&mut body)
        .expect("request body should read");
    buffer.extend_from_slice(&body);

    String::from_utf8(buffer).expect("request should be utf8")
}

fn write_json_response(stream: &mut std::net::TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(response.as_bytes())
        .expect("response should write");
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
