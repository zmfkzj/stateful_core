use std::{fs, path::PathBuf};

#[cfg(unix)]
use std::os::unix::net::UnixListener;

use stateful_cli::{ExternalRunRequest, GlobalPaths, SandboxNetworkPolicy, request_external_run};

#[test]
fn external_run_request_rejects_internal_target_after_normalization() {
    let root = temp_root("internal-target");
    let repo_root = root.join("repo");
    fs::create_dir_all(&repo_root).expect("repo root should be created");
    fs::write(repo_root.join("README.md"), "docs").expect("repo file should be created");

    let error = request_external_run(ExternalRunRequest {
        repo_root: repo_root.clone(),
        purpose: "try to mutate repo through external-run".to_string(),
        command: "printf changed > README.md".to_string(),
        write_targets: vec!["../repo/README.md".to_string()],
        create_targets: Vec::new(),
        write_dirs: Vec::new(),
        connect_sockets: Vec::new(),
        allow_signal: false,
        network: SandboxNetworkPolicy::Disabled,
        timeout_seconds: None,
    })
    .expect_err("external-run should reject normalized internal targets");

    assert!(error.to_string().contains("outside the repo"));
}

#[test]
fn external_run_request_executes_external_directory_command() {
    if std::env::var_os("STATEFUL_SANDBOX_RUN_ACTIVE").is_some() {
        return;
    }

    let root = temp_root("request-run");
    let repo_root = root.join("repo");
    let external_dir = root.join("bin");
    let paths = GlobalPaths::new(root.join("home"));
    fs::create_dir_all(&repo_root).expect("repo root should be created");
    fs::create_dir_all(&external_dir).expect("external dir should be created");
    let output_path = external_dir.join("stateful");

    let output = request_external_run(ExternalRunRequest {
        repo_root,
        purpose: "write external test file".to_string(),
        command: format!("printf ok > {}", shell_quote_path(&output_path)),
        write_targets: Vec::new(),
        create_targets: Vec::new(),
        write_dirs: vec![external_dir.to_string_lossy().to_string()],
        connect_sockets: Vec::new(),
        allow_signal: false,
        network: SandboxNetworkPolicy::Disabled,
        timeout_seconds: Some(10),
    })
    .expect("external-run request should execute");

    assert_eq!(output.status, "exited");
    assert_eq!(output.exit_code, Some(0));
    assert!(
        !paths.home.join("external-run/requests").exists(),
        "external-run request should not create pending approval records"
    );
    assert_eq!(
        fs::read_to_string(output_path).expect("external file should be written"),
        "ok"
    );
}

#[test]
#[cfg(unix)]
fn external_run_request_accepts_connect_socket_without_write_scope() {
    if std::env::var_os("STATEFUL_SANDBOX_RUN_ACTIVE").is_some() {
        return;
    }

    let root = temp_root("socket-scope");
    let repo_root = root.join("repo");
    let socket_dir = root.join("socket");
    fs::create_dir_all(&repo_root).expect("repo root should be created");
    fs::create_dir_all(&socket_dir).expect("socket dir should be created");
    let socket_path = socket_dir.join("daemon.sock");
    let _listener = UnixListener::bind(&socket_path).expect("unix socket should be created");

    let output = request_external_run(ExternalRunRequest {
        repo_root,
        purpose: "connect to external control socket".to_string(),
        command: "true".to_string(),
        write_targets: Vec::new(),
        create_targets: Vec::new(),
        write_dirs: Vec::new(),
        connect_sockets: vec![socket_path.to_string_lossy().to_string()],
        allow_signal: false,
        network: SandboxNetworkPolicy::Disabled,
        timeout_seconds: Some(10),
    })
    .expect("external-run request should accept socket-only scope");

    assert_eq!(output.status, "exited");
    assert_eq!(output.exit_code, Some(0));
}

#[test]
#[cfg(unix)]
fn external_run_request_rejects_non_socket_connect_scope() {
    let root = temp_root("non-socket-scope");
    let repo_root = root.join("repo");
    let external_dir = root.join("external");
    fs::create_dir_all(&repo_root).expect("repo root should be created");
    fs::create_dir_all(&external_dir).expect("external dir should be created");
    let file_path = external_dir.join("not-a-socket");
    fs::write(&file_path, "plain file").expect("plain file should be created");

    let error = request_external_run(ExternalRunRequest {
        repo_root,
        purpose: "connect to external control socket".to_string(),
        command: "true".to_string(),
        write_targets: Vec::new(),
        create_targets: Vec::new(),
        write_dirs: Vec::new(),
        connect_sockets: vec![file_path.to_string_lossy().to_string()],
        allow_signal: false,
        network: SandboxNetworkPolicy::Disabled,
        timeout_seconds: Some(10),
    })
    .expect_err("external-run should reject non-socket connect scopes");

    assert!(error.to_string().contains("must be a Unix socket"));
}

fn temp_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "stateful-external-run-{label}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("temp root should be created");
    root
}

fn shell_quote_path(path: &std::path::Path) -> String {
    let value = path.to_string_lossy();
    let mut quoted = String::from("'");
    for ch in value.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}
