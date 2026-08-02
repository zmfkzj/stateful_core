use stateful_cli::{
    CommandIdentity, CommitRequest, ServerRuntime, post_command, process_start_identity_for_pid,
    run_structured_commit, write_runtime_file,
};
use stateful_core::{
    ActorType, AgentIdentity, CoordinationSettings, SourceKind, SourceRef, WorkspaceIdentity,
};
use std::{
    fs,
    net::{TcpListener, TcpStream},
    path::Path,
    process::Command,
    sync::{Arc, Barrier},
    thread,
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
fn structured_commit_preserves_unrelated_staged_state() {
    let root = git_repo("stateful-commit-index-preservation");
    seed(&root, &["docs/plan.md", "docs/other.md"]);
    fs::write(root.path().join("docs/plan.md"), "plan v2\n")
        .expect("test operation should succeed");
    fs::write(root.path().join("docs/other.md"), "caller staged\n")
        .expect("test operation should succeed");
    git(root.path(), &["add", "docs/other.md"]);

    let server = TestServer::start();
    let identity = start_task(&server, root.path(), "task-index", "agent-index");
    install_runtime(root.path(), &server.runtime);
    run_structured_commit(request(root.path(), identity, &["docs/plan.md"]))
        .expect("explicit target should commit around caller-owned index state");

    assert_eq!(
        git_output(root.path(), &["show", "HEAD:docs/plan.md"]),
        "plan v2\n"
    );
    assert_eq!(
        git_output(root.path(), &["diff", "--cached", "--name-only"]),
        "docs/other.md\n"
    );
    assert_eq!(
        git_output(root.path(), &["show", ":docs/other.md"]),
        "caller staged\n"
    );
}

#[test]
fn structured_commits_serialize_on_git_metadata() {
    let root = git_repo("stateful-commit-serialization");
    seed(&root, &["docs/one.md", "docs/two.md"]);
    fs::write(root.path().join("docs/one.md"), "one v2\n").expect("test operation should succeed");
    fs::write(root.path().join("docs/two.md"), "two v2\n").expect("test operation should succeed");

    let server = Arc::new(TestServer::start());
    install_runtime(root.path(), &server.runtime);
    let first = start_task(&server, root.path(), "task-one", "agent-one");
    let second = start_task(&server, root.path(), "task-two", "agent-two");
    let barrier = Arc::new(Barrier::new(2));
    let run = |identity: CommandIdentity,
               path: &'static str,
               barrier: Arc<Barrier>,
               server: Arc<TestServer>| {
        let root = root.path().to_path_buf();
        thread::spawn(move || {
            barrier.wait();
            let result = run_structured_commit(request(&root, identity.clone(), &[path]));
            finish_task(&server, identity);
            result
        })
    };
    let one = run(
        first,
        "docs/one.md",
        Arc::clone(&barrier),
        Arc::clone(&server),
    );
    let two = run(second, "docs/two.md", barrier, server);
    one.join()
        .expect("test operation should succeed")
        .expect("first commit should succeed");
    two.join()
        .expect("test operation should succeed")
        .expect("queued commit should activate and succeed");
    assert_eq!(
        git_output(root.path(), &["rev-list", "--count", "HEAD"]),
        "3\n"
    );
}

#[cfg(unix)]
#[test]
fn structured_commit_head_cas_rejects_hook_ref_update() {
    let root = git_repo("stateful-commit-head-cas");
    seed(&root, &["docs/plan.md"]);
    fs::write(root.path().join("docs/plan.md"), "plan v2\n")
        .expect("test operation should succeed");
    git(
        root.path(),
        &["commit", "--allow-empty", "-m", "second seed"],
    );
    write_hook(
        root.path(),
        "pre-commit",
        "#!/bin/sh\ngit update-ref HEAD HEAD^\n",
    );
    let server = TestServer::start();
    let identity = start_task(&server, root.path(), "task-head", "agent-head");
    install_runtime(root.path(), &server.runtime);
    let head_before = git_output(root.path(), &["rev-parse", "HEAD"]);
    let error = run_structured_commit(request(root.path(), identity, &["docs/plan.md"]))
        .expect_err("ref mutation must block the pending HEAD CAS");
    assert!(error.to_string().contains("pre-commit hook"));
    assert_eq!(git_output(root.path(), &["rev-parse", "HEAD"]), head_before);
}

#[cfg(unix)]
#[test]
fn only_pre_commit_and_commit_msg_run_in_private_exact_staging() {
    let root = git_repo("stateful-commit-hooks");
    seed(&root, &["docs/plan.md"]);
    fs::write(root.path().join("docs/plan.md"), "plan v2\n")
        .expect("test operation should succeed");
    write_hook(
        root.path(),
        "pre-commit",
        "#!/bin/sh\nprintf pre >> docs/plan.md\n",
    );
    write_hook(
        root.path(),
        "commit-msg",
        "#!/bin/sh\nprintf msg >> docs/plan.md\n",
    );
    let prepare_marker = root.path().join(".prepare-ran");
    let post_marker = root.path().join(".post-ran");
    write_hook(
        root.path(),
        "prepare-commit-msg",
        &format!("#!/bin/sh\nprintf bad > {}\n", prepare_marker.display()),
    );
    write_hook(
        root.path(),
        "post-commit",
        &format!("#!/bin/sh\nprintf bad > {}\n", post_marker.display()),
    );

    let server = TestServer::start();
    let identity = start_task(&server, root.path(), "task-hooks", "agent-hooks");
    install_runtime(root.path(), &server.runtime);
    run_structured_commit(request(root.path(), identity, &["docs/plan.md"]))
        .expect("allowed hooks should run under private exact staging");

    assert_eq!(
        git_output(root.path(), &["show", "HEAD:docs/plan.md"]),
        "plan v2\npremsg"
    );
    assert!(!prepare_marker.exists());
    assert!(!post_marker.exists());
}

#[cfg(unix)]
#[test]
fn undeclared_hook_workspace_write_blocks_commit_before_ref_update() {
    let root = git_repo("stateful-commit-hook-escape");
    seed(&root, &["docs/plan.md"]);
    fs::write(root.path().join("docs/plan.md"), "plan v2\n")
        .expect("test operation should succeed");
    let generated = root.path().join("generated.txt");
    write_hook(
        root.path(),
        "pre-commit",
        &format!("#!/bin/sh\nprintf generated > {}\n", generated.display()),
    );
    let server = TestServer::start();
    let identity = start_task(
        &server,
        root.path(),
        "task-hook-escape",
        "agent-hook-escape",
    );
    install_runtime(root.path(), &server.runtime);
    let error = run_structured_commit(request(root.path(), identity, &["docs/plan.md"]))
        .expect_err("undeclared hook write must fail before commit");
    assert!(error.to_string().contains("pre-commit hook"));
    assert!(
        !generated.exists(),
        "sandbox must block the write before creation"
    );
    assert_eq!(
        git_output(root.path(), &["rev-list", "--count", "HEAD"]),
        "1\n"
    );
}

#[test]
fn structured_commit_reports_primary_index_sync_failure_after_head_update() {
    let root = git_repo("stateful-commit-sync-warning");
    seed(&root, &["docs/plan.md"]);
    fs::write(root.path().join("docs/plan.md"), "plan v2\n")
        .expect("test operation should succeed");
    fs::write(root.path().join(".git/index.lock"), "").expect("test operation should succeed");

    let server = TestServer::start();
    let identity = start_task(
        &server,
        root.path(),
        "task-sync-warning",
        "agent-sync-warning",
    );
    install_runtime(root.path(), &server.runtime);
    let result = run_structured_commit(request(root.path(), identity, &["docs/plan.md"]))
        .expect("HEAD CAS makes a primary-index synchronization failure non-terminal");

    assert_eq!(
        result.commit_sha,
        git_output(root.path(), &["rev-parse", "HEAD"]).trim()
    );
    assert_eq!(
        git_output(root.path(), &["rev-list", "--count", "HEAD"]),
        "2\n"
    );
    assert!(result.warnings.iter().any(|warning| {
        warning.contains("primary index synchronization after HEAD update failed")
    }));
}

#[test]
fn structured_commit_rejects_broad_pathspecs() {
    let root = git_repo("stateful-commit-broad-path");
    seed(&root, &["docs/plan.md"]);
    let server = TestServer::start();
    let identity = start_task(&server, root.path(), "task-broad", "agent-broad");
    install_runtime(root.path(), &server.runtime);

    let error = run_structured_commit(request(root.path(), identity, &["docs"]))
        .expect_err("directory pathspec must be rejected");
    assert!(
        error.to_string().contains("explicit file paths"),
        "{error:#}"
    );
}

#[test]
fn structured_commit_commits_deleted_tracked_target() {
    let root = git_repo("stateful-commit-deleted-target");
    seed(&root, &["docs/plan.md"]);
    fs::remove_file(root.path().join("docs/plan.md")).expect("test operation should succeed");
    let server = TestServer::start();
    let identity = start_task(&server, root.path(), "task-delete", "agent-delete");
    install_runtime(root.path(), &server.runtime);

    run_structured_commit(request(root.path(), identity, &["docs/plan.md"]))
        .expect("deleted tracked target should commit");
    let output = Command::new("git")
        .args(["cat-file", "-e", "HEAD:docs/plan.md"])
        .current_dir(root.path())
        .output()
        .expect("test operation should succeed");
    assert!(!output.status.success());
}

#[test]
fn structured_commit_commits_new_explicit_file() {
    let root = git_repo("stateful-commit-new-target");
    seed(&root, &["README.md"]);
    fs::create_dir_all(root.path().join("docs")).expect("test operation should succeed");
    fs::write(root.path().join("docs/plan.md"), "new plan\n")
        .expect("test operation should succeed");
    let server = TestServer::start();
    let identity = start_task(&server, root.path(), "task-new", "agent-new");
    install_runtime(root.path(), &server.runtime);

    run_structured_commit(request(root.path(), identity, &["docs/plan.md"]))
        .expect("new explicit file should commit");
    assert_eq!(
        git_output(root.path(), &["show", "HEAD:docs/plan.md"]),
        "new plan\n"
    );
}

fn request(root: &Path, identity: CommandIdentity, paths: &[&str]) -> CommitRequest {
    CommitRequest {
        repo_root: root.to_path_buf(),
        message: "docs: update plan".to_string(),
        paths: paths.iter().map(|path| (*path).to_string()).collect(),
        identity,
    }
}

struct TestServer {
    runtime: ServerRuntime,
}

impl TestServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test operation should succeed");
        listener
            .set_nonblocking(true)
            .expect("test operation should succeed");
        let address = listener
            .local_addr()
            .expect("test operation should succeed");
        let token = format!("token-{}", uuid::Uuid::new_v4());
        let thread_token = token.clone();
        thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("test operation should succeed");
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener)
                    .expect("test operation should succeed");
                stateful_server::serve_listener(
                    listener,
                    stateful_server::ServerConfig::new(thread_token),
                )
                .await
                .expect("test operation should succeed");
            });
        });
        for _ in 0..100 {
            if TcpStream::connect(address).is_ok() {
                return Self {
                    runtime: ServerRuntime::new(
                        format!("http://{address}"),
                        token,
                        "workspace-commit",
                        std::process::id(),
                        process_start_identity_for_pid(std::process::id())
                            .expect("current process should have an identity"),
                    ),
                };
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("test server did not become ready");
    }
}

fn install_runtime(root: &Path, runtime: &ServerRuntime) {
    write_runtime_file(root, runtime).expect("test operation should succeed");
}

fn start_task(server: &TestServer, root: &Path, task: &str, agent: &str) -> CommandIdentity {
    let identity = identity(root, task, agent);
    let _: stateful_store::TaskCommandResult = post_command(
        &server.runtime,
        "/v2/tasks/start",
        &identity,
        &stateful_store::TaskStartInput {
            next_action: "commit".to_string(),
            settings: CoordinationSettings {
                inactivity_timeout_seconds: 90,
                ..CoordinationSettings::default()
            },
            expires_at: future_timestamp(),
            runtime_process: None,
        },
    )
    .expect("test operation should succeed");
    identity
}

fn finish_task(server: &TestServer, identity: CommandIdentity) {
    let identity = CommandIdentity::new_now(
        identity.task_id,
        uuid::Uuid::new_v4().to_string(),
        identity.agent,
        identity.workspace,
        identity.source,
    );
    let _: stateful_store::TaskCommandResult = post_command(
        &server.runtime,
        "/v2/tasks/finalize",
        &identity,
        &stateful_store::TaskEndInput { handoff: None },
    )
    .expect("test operation should succeed");
}

fn identity(root: &Path, task: &str, agent: &str) -> CommandIdentity {
    CommandIdentity::new_now(
        task,
        uuid::Uuid::new_v4().to_string(),
        AgentIdentity {
            agent_id: agent.to_string(),
            turn_id: None,
            actor_id: agent.to_string(),
            actor_type: ActorType::Agent,
            owner_id: None,
            parent_agent_id: None,
            parent_actor_id: None,
        },
        WorkspaceIdentity {
            root: root.to_string_lossy().to_string(),
            workspace_id: "workspace-commit".to_string(),
            repo_id: "repo-commit".to_string(),
            worktree_id: "worktree-commit".to_string(),
            branch: "main".to_string(),
        },
        SourceRef {
            kind: SourceKind::Cli,
            event: "commit_test".to_string(),
            tool_name: None,
            source_ref: "test".to_string(),
        },
    )
}

fn future_timestamp() -> String {
    use time::{Duration as TimeDuration, OffsetDateTime, format_description::well_known::Rfc3339};
    (OffsetDateTime::now_utc() + TimeDuration::seconds(30))
        .format(&Rfc3339)
        .expect("test operation should succeed")
}

fn git_repo(name: &str) -> tempfile::TempDir {
    let root = tempfile::Builder::new()
        .prefix(name)
        .tempdir()
        .expect("test operation should succeed");
    git(root.path(), &["init"]);
    git(root.path(), &["config", "user.name", "stateful test"]);
    git(
        root.path(),
        &["config", "user.email", "stateful@example.invalid"],
    );
    root
}

fn seed(root: &tempfile::TempDir, paths: &[&str]) {
    for path in paths {
        let path = root.path().join(path);
        fs::create_dir_all(path.parent().expect("seed path should have a parent"))
            .expect("test operation should succeed");
        fs::write(path, "base\n").expect("test operation should succeed");
    }
    git(root.path(), &["add", "."]);
    git(root.path(), &["commit", "-m", "seed"]);
}

#[cfg(unix)]
fn write_hook(root: &Path, name: &str, script: &str) {
    let path = root.join(".git/hooks").join(name);
    fs::write(&path, script).expect("test operation should succeed");
    let mut permissions = fs::metadata(&path)
        .expect("test operation should succeed")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("test operation should succeed");
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

fn git_output(root: &Path, args: &[&str]) -> String {
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
    String::from_utf8(output.stdout).expect("test operation should succeed")
}
