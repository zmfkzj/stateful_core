use std::{fs, path::PathBuf};

use stateful_cli::{
    ExternalRunRequest, GlobalPaths, SandboxNetworkPolicy, approve_external_run,
    request_external_run, run_approved_external_run,
};

#[test]
fn external_run_request_rejects_internal_target_after_normalization() {
    let root = temp_root("internal-target");
    let repo_root = root.join("repo");
    let paths = GlobalPaths::new(root.join("home"));
    fs::create_dir_all(&repo_root).expect("repo root should be created");
    fs::write(repo_root.join("README.md"), "docs").expect("repo file should be created");

    let error = request_external_run(ExternalRunRequest {
        repo_root: repo_root.clone(),
        paths,
        purpose: "try to mutate repo through external-run".to_string(),
        command: "printf changed > README.md".to_string(),
        write_targets: vec!["../repo/README.md".to_string()],
        create_targets: Vec::new(),
        write_dirs: Vec::new(),
        network: SandboxNetworkPolicy::Disabled,
        timeout_seconds: None,
    })
    .expect_err("external-run should reject normalized internal targets");

    assert!(error.to_string().contains("outside the repo"));
}

#[test]
fn external_run_request_returns_copy_paste_approval_guidance() {
    let root = temp_root("guidance");
    let repo_root = root.join("repo");
    let external_dir = root.join("bin");
    let paths = GlobalPaths::new(root.join("home"));
    fs::create_dir_all(&repo_root).expect("repo root should be created");
    fs::create_dir_all(&external_dir).expect("external dir should be created");

    let approval = request_external_run(ExternalRunRequest {
        repo_root,
        paths,
        purpose: "install rebuilt binaries".to_string(),
        command: "install -m 755 target/release/stateful /tmp/stateful".to_string(),
        write_targets: Vec::new(),
        create_targets: Vec::new(),
        write_dirs: vec![external_dir.to_string_lossy().to_string()],
        network: SandboxNetworkPolicy::Disabled,
        timeout_seconds: Some(10),
    })
    .expect("request should be recorded");

    assert!(approval.guidance.contains("External run approval required"));
    assert!(approval.guidance.contains("install rebuilt binaries"));
    assert!(approval.guidance.contains(&approval.request_id));
    assert!(approval.guidance.contains("Copy and paste this command"));
    assert!(approval.guidance.contains("external-run approve"));
    assert!(approval.guidance.contains("--run"));
    assert!(
        approval
            .guidance
            .contains(&external_dir.to_string_lossy().to_string())
    );
}

#[test]
fn external_run_request_guidance_summarizes_approval_inputs_by_flag() {
    let root = temp_root("guidance-summary");
    let repo_root = root.join("repo");
    let external_dir = root.join("bin");
    let existing_file = external_dir.join("stateful");
    let created_file = external_dir.join("stateful.new");
    let paths = GlobalPaths::new(root.join("home"));
    fs::create_dir_all(&repo_root).expect("repo root should be created");
    fs::create_dir_all(&external_dir).expect("external dir should be created");
    fs::write(&existing_file, "old").expect("external file should be created");

    let command = format!(
        "install -m 755 target/release/stateful {}",
        shell_quote_path(&created_file)
    );
    let approval = request_external_run(ExternalRunRequest {
        repo_root,
        paths,
        purpose: "install rebuilt binaries".to_string(),
        command: command.clone(),
        write_targets: vec![existing_file.to_string_lossy().to_string()],
        create_targets: vec![created_file.to_string_lossy().to_string()],
        write_dirs: vec![external_dir.to_string_lossy().to_string()],
        network: SandboxNetworkPolicy::Disabled,
        timeout_seconds: Some(10),
    })
    .expect("request should be recorded");
    let canonical_external_dir =
        fs::canonicalize(&external_dir).expect("external dir should canonicalize");
    let canonical_existing_file =
        fs::canonicalize(&existing_file).expect("existing file should canonicalize");
    let canonical_created_file = canonical_external_dir.join("stateful.new");

    assert!(approval.guidance.contains("External run request details:"));
    assert!(
        approval
            .guidance
            .contains("--purpose: \"install rebuilt binaries\"")
    );
    assert!(approval.guidance.contains(&format!(
        "--write-target: {}",
        canonical_existing_file.display()
    )));
    assert!(approval.guidance.contains(&format!(
        "--create-target: {}",
        canonical_created_file.display()
    )));
    assert!(approval.guidance.contains(&format!(
        "--write-dir: {}",
        canonical_external_dir.display()
    )));
    assert!(
        approval
            .guidance
            .contains(&format!("--command: {:?}", command))
    );
}

#[test]
fn external_run_request_guidance_escapes_multiline_approval_inputs() {
    let root = temp_root("guidance-escaped");
    let repo_root = root.join("repo");
    let external_dir = root.join("bin");
    let paths = GlobalPaths::new(root.join("home"));
    fs::create_dir_all(&repo_root).expect("repo root should be created");
    fs::create_dir_all(&external_dir).expect("external dir should be created");

    let approval = request_external_run(ExternalRunRequest {
        repo_root,
        paths,
        purpose: "install\n--write-dir: /spoofed".to_string(),
        command: "printf ok\n--purpose: spoofed".to_string(),
        write_targets: Vec::new(),
        create_targets: Vec::new(),
        write_dirs: vec![external_dir.to_string_lossy().to_string()],
        network: SandboxNetworkPolicy::Disabled,
        timeout_seconds: Some(10),
    })
    .expect("request should be recorded");

    assert!(
        approval
            .guidance
            .contains("--purpose: \"install\\n--write-dir: /spoofed\"")
    );
    assert!(
        approval
            .guidance
            .contains("--command: \"printf ok\\n--purpose: spoofed\"")
    );
    assert!(
        !approval.guidance.contains("install\n--write-dir: /spoofed"),
        "guidance should not render raw multiline purpose values"
    );
    assert!(
        !approval.guidance.contains("printf ok\n--purpose: spoofed"),
        "guidance should not render raw multiline command values"
    );
}

#[test]
fn approved_external_run_can_write_external_directory() {
    if std::env::var_os("STATEFUL_SANDBOX_RUN_ACTIVE").is_some() {
        return;
    }

    let root = temp_root("approved-run");
    let repo_root = root.join("repo");
    let external_dir = root.join("bin");
    let paths = GlobalPaths::new(root.join("home"));
    fs::create_dir_all(&repo_root).expect("repo root should be created");
    fs::create_dir_all(&external_dir).expect("external dir should be created");
    let output_path = external_dir.join("stateful");

    let approval = request_external_run(ExternalRunRequest {
        repo_root,
        paths: paths.clone(),
        purpose: "write external test file".to_string(),
        command: format!("printf ok > {}", shell_quote_path(&output_path)),
        write_targets: Vec::new(),
        create_targets: Vec::new(),
        write_dirs: vec![external_dir.to_string_lossy().to_string()],
        network: SandboxNetworkPolicy::Disabled,
        timeout_seconds: Some(10),
    })
    .expect("request should be recorded");

    approve_external_run(&paths, &approval.request_id, false).expect("request should approve");
    let output = run_approved_external_run(&paths, &approval.request_id)
        .expect("approved external run should execute");

    assert_eq!(output.status, "exited");
    assert_eq!(output.exit_code, Some(0));
    assert_eq!(
        fs::read_to_string(output_path).expect("external file should be written"),
        "ok"
    );
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
