use std::{
    fs,
    process::Command,
    time::{Duration, Instant},
};

use stateful_cli::{ResultParser, ValidationConfig, ValidationStatus, run_validation_profile};

const PROFILE_YAML: &str = r#"
profiles:
  - profile_id: unit
    description: Run unit tests
    command: cargo test -p stateful-core
    cwd: .
    timeout_seconds: 60
    allowed_writes:
      - target/**
    denied_writes:
      - src/**
    exclusive: true
    result_parser: exit_code
"#;

#[test]
fn validation_profile_loads_exit_code_runner() {
    let config =
        ValidationConfig::from_yaml(PROFILE_YAML).expect("validation profile yaml should parse");
    let profile = config.profile("unit").expect("unit profile should exist");

    assert_eq!(profile.result_parser, ResultParser::ExitCode);
    assert_eq!(profile.command, "cargo test -p stateful-core");
    assert!(profile.denied_writes.iter().any(|path| path == "src/**"));
    assert!(profile.exclusive);
}

#[test]
fn validation_runner_reports_passed_for_successful_command() {
    let repo = TestRepo::new("validation-pass");
    repo.write_validation(
        r#"
profiles:
  - profile_id: pass
    description: Pass
    command: true
    cwd: .
    timeout_seconds: 10
    denied_writes:
      - src/**
"#,
    );

    let result = run_validation_profile(repo.path(), "pass").expect("validation should run");

    assert_eq!(result.status, ValidationStatus::Passed);
}

#[test]
fn validation_runner_reports_failed_for_nonzero_command() {
    let repo = TestRepo::new("validation-fail");
    repo.write_validation(
        r#"
profiles:
  - profile_id: fail
    description: Fail
    command: false
    cwd: .
    timeout_seconds: 10
    denied_writes:
      - src/**
"#,
    );

    let result = run_validation_profile(repo.path(), "fail").expect("validation should run");

    assert_eq!(result.status, ValidationStatus::Failed);
}

#[test]
fn validation_runner_errors_when_denied_path_is_dirty_before_run() {
    let repo = TestRepo::new("validation-dirty-before");
    repo.write_file("src/auth.ts", "changed before validation\n");
    repo.write_validation(
        r#"
profiles:
  - profile_id: unit
    description: Unit
    command: true
    cwd: .
    timeout_seconds: 10
    denied_writes:
      - src/**
"#,
    );

    let result = run_validation_profile(repo.path(), "unit").expect("validation should run");

    assert_eq!(result.status, ValidationStatus::Error);
    assert!(
        result
            .message
            .contains("denied path dirty before validation")
    );
}

#[test]
fn validation_runner_reports_failed_policy_for_new_denied_write() {
    let repo = TestRepo::new("validation-policy-fail");
    repo.write_validation(
        r#"
profiles:
  - profile_id: unit
    description: Unit
    command: mkdir -p src && printf changed > src/generated.ts
    cwd: .
    timeout_seconds: 10
    denied_writes:
      - src/**
"#,
    );

    let result = run_validation_profile(repo.path(), "unit").expect("validation should run");

    assert_eq!(result.status, ValidationStatus::FailedPolicy);
}

#[test]
fn validation_runner_times_out_long_running_command() {
    let repo = TestRepo::new("validation-timeout");
    repo.write_validation(
        r#"
profiles:
  - profile_id: slow
    description: Slow
    command: sleep 5
    cwd: .
    timeout_seconds: 1
    denied_writes:
      - src/**
"#,
    );

    let started = Instant::now();
    let result = run_validation_profile(repo.path(), "slow").expect("validation should run");

    assert_eq!(result.status, ValidationStatus::Timeout);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "timeout should terminate the command promptly"
    );
}

struct TestRepo {
    root: std::path::PathBuf,
}

impl TestRepo {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        if root.exists() {
            fs::remove_dir_all(&root).expect("old temp repo should be removable");
        }
        fs::create_dir_all(root.join("src")).expect("src dir should be creatable");
        fs::create_dir_all(root.join(".stateful")).expect("stateful dir should be creatable");
        fs::write(root.join("src/auth.ts"), "initial\n").expect("source file should write");
        run_git(&root, ["init"]);
        run_git(&root, ["add", "."]);
        run_git(&root, ["commit", "-m", "initial"]);

        Self { root }
    }

    fn path(&self) -> &std::path::Path {
        &self.root
    }

    fn write_file(&self, path: &str, contents: &str) {
        fs::write(self.root.join(path), contents).expect("test file should write");
    }

    fn write_validation(&self, contents: &str) {
        fs::write(self.root.join(".stateful/validation.yml"), contents)
            .expect("validation config should write");
    }
}

impl Drop for TestRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn run_git<const N: usize>(root: &std::path::Path, args: [&str; N]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "stateful test")
        .env("GIT_AUTHOR_EMAIL", "stateful@example.invalid")
        .env("GIT_COMMITTER_NAME", "stateful test")
        .env("GIT_COMMITTER_EMAIL", "stateful@example.invalid")
        .status()
        .expect("git command should run");
    assert!(status.success(), "git command should succeed");
}
