use stateful_cli::GlobalPaths;
use std::{path::PathBuf, process::Command};

const CHILD_CASE: &str = "STATEFUL_GLOBAL_PATHS_CHILD_CASE";
const EXPECTED_HOME: &str = "STATEFUL_GLOBAL_PATHS_EXPECTED_HOME";
const EXPECTED_ERROR: &str = "STATEFUL_GLOBAL_PATHS_EXPECTED_ERROR";

#[test]
fn global_paths_are_rooted_under_stateful_home() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let home = temp.path().join("stateful-home");
    let paths = GlobalPaths::new(&home);

    assert_eq!(paths.home, home);
    assert_eq!(paths.config_yml, paths.home.join("config.yml"));
    assert_eq!(paths.state_db, paths.home.join("state.db"));
    assert_eq!(paths.runtime_dir, paths.home.join("runtime"));
    assert_eq!(
        paths.server_json,
        paths.home.join("runtime").join("server.json")
    );
    assert_eq!(
        paths.server_lock,
        paths.home.join("runtime").join("server.lock")
    );
    assert_eq!(
        paths.server_log,
        paths.home.join("runtime").join("server.log")
    );
    assert_eq!(paths.repos_dir, paths.home.join("repos"));
    assert_eq!(paths.outbox_dir, paths.home.join("outbox"));
}

#[test]
fn from_env_prefers_stateful_home_over_home() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let temp_dir = temp.path();
    let stateful_home = temp_dir.join("stateful-home");
    let home = temp_dir.join("home");

    run_from_env_child(|command| {
        command
            .env("STATEFUL_HOME", &stateful_home)
            .env("HOME", &home)
            .env(CHILD_CASE, "expect_home")
            .env(EXPECTED_HOME, &stateful_home);
    });
}

#[test]
fn from_env_falls_back_to_home_stateful_core() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let home = temp.path().join("stateful-home");

    run_from_env_child(|command| {
        command
            .env("HOME", &home)
            .env(CHILD_CASE, "expect_home")
            .env(EXPECTED_HOME, home.join(".stateful_core"));
    });
}

#[test]
fn from_env_errors_without_stateful_home_or_home() {
    run_from_env_child(|command| {
        command
            .env(CHILD_CASE, "expect_error")
            .env(EXPECTED_ERROR, "HOME is not set; set STATEFUL_HOME");
    });
}

#[test]
fn from_env_rejects_empty_stateful_home() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let home = temp.path().join("stateful-home");

    run_from_env_child(|command| {
        command
            .env("STATEFUL_HOME", "")
            .env("HOME", &home)
            .env(CHILD_CASE, "expect_error")
            .env(EXPECTED_ERROR, "STATEFUL_HOME is set but empty");
    });
}

#[test]
fn from_env_rejects_empty_home() {
    run_from_env_child(|command| {
        command
            .env("HOME", "")
            .env(CHILD_CASE, "expect_error")
            .env(EXPECTED_ERROR, "HOME is set but empty; set STATEFUL_HOME");
    });
}

fn run_from_env_child(configure: impl FnOnce(&mut Command)) {
    let mut command = Command::new(std::env::current_exe().expect("current test binary path"));
    command
        .arg("from_env_child_probe")
        .arg("--ignored")
        .arg("--exact")
        .arg("--nocapture")
        .env_clear();
    configure(&mut command);

    let output = command.output().expect("from_env child test should run");
    assert!(
        output.status.success(),
        "from_env child failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore]
fn from_env_child_probe() {
    let Ok(child_case) = std::env::var(CHILD_CASE) else {
        return;
    };

    match child_case.as_str() {
        "expect_home" => {
            let expected_home = PathBuf::from(
                std::env::var_os(EXPECTED_HOME).expect("expected home must be configured"),
            );
            let paths = GlobalPaths::from_env().expect("from_env should resolve paths");

            assert_eq!(paths.home, expected_home);
        }
        "expect_error" => {
            let expected_error =
                std::env::var(EXPECTED_ERROR).expect("expected error must be configured");
            let error = GlobalPaths::from_env().expect_err("from_env should return an error");

            assert!(
                error.to_string().contains(&expected_error),
                "expected error to contain {expected_error:?}, got {error}"
            );
        }
        other => panic!("unknown child case {other:?}"),
    }
}
