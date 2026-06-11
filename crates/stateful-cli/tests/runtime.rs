use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::Command,
    sync::mpsc,
    thread,
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use stateful_cli::{
    CurrentSession, GlobalPaths, IntentCancelArgs, IntentClaimArgs, IntentDeclareArgs,
    IntentRequestArgs, ServerRuntime, cancel_intent_via_http, claim_intent_via_http,
    declare_intent_via_http, discover_runtime, discover_runtime_with_global, get_json,
    global_state_db_path, post_json, read_current_session_file,
    read_current_session_file_for_session, request_intent_via_http, state_db_path,
    write_current_session_file, write_current_session_file_for_session, write_global_runtime_file,
    write_runtime_file,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const CURRENT_SESSION_CHILD_CASE: &str = "STATEFUL_RUNTIME_CURRENT_SESSION_CHILD_CASE";
const CURRENT_SESSION_CHILD_ROOT: &str = "STATEFUL_RUNTIME_CURRENT_SESSION_CHILD_ROOT";

fn run_current_session_child(repo_root: &Path, child_case: &str) {
    let output = Command::new(std::env::current_exe().expect("current test binary path"))
        .arg("current_session_file_child_probe")
        .arg("--ignored")
        .arg("--exact")
        .arg("--nocapture")
        .env_clear()
        .env(CURRENT_SESSION_CHILD_CASE, child_case)
        .env(CURRENT_SESSION_CHILD_ROOT, repo_root)
        .output()
        .expect("current session child test should run");
    assert!(
        output.status.success(),
        "current session child failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore]
fn current_session_file_child_probe() {
    let Ok(child_case) = std::env::var(CURRENT_SESSION_CHILD_CASE) else {
        return;
    };
    let repo_root = std::path::PathBuf::from(
        std::env::var_os(CURRENT_SESSION_CHILD_ROOT)
            .expect("current session child root must be configured"),
    );

    match child_case.as_str() {
        "roundtrip_no_codex_env" => {
            write_current_session_file(&repo_root, &CurrentSession::new("s1", "w1"))
                .expect("current session file should write");

            let session =
                read_current_session_file(&repo_root).expect("current session should read");
            assert_eq!(session.session_id, "s1");
            assert_eq!(session.workspace_id, "w1");
        }
        "read_prefers_codex_thread_id" => {
            let session =
                read_current_session_file(&repo_root).expect("current session should read");
            assert_eq!(session.session_id, "thread-child");
            assert_eq!(session.workspace_id, "w1");
        }
        "read_uses_legacy_session" => {
            let session =
                read_current_session_file(&repo_root).expect("current session should read");
            assert_eq!(session.session_id, "legacy-session");
            assert_eq!(session.workspace_id, "w1");
        }
        "read_rejects_ambiguous_legacy_session" => {
            let error = read_current_session_file(&repo_root)
                .expect_err("ambiguous legacy session should fail");
            assert!(
                error
                    .to_string()
                    .contains("multiple session-bound current-session files")
            );
        }
        "read_rejects_unverified_legacy_session" => {
            let error = read_current_session_file(&repo_root)
                .expect_err("unverified legacy session should fail");
            assert!(
                error
                    .to_string()
                    .contains("has no matching session-bound file")
            );
        }
        "read_uses_stateful_session_id" => {
            let session =
                read_current_session_file(&repo_root).expect("current session should read");
            assert_eq!(session.session_id, "session-child");
            assert_eq!(session.workspace_id, "w1");
        }
        "read_symlink_error" => {
            let error =
                read_current_session_file(&repo_root).expect_err("symlinked session should fail");
            assert!(error.to_string().contains("symlinked current session file"));
        }
        "write_symlink_error" => {
            let error = write_current_session_file(&repo_root, &CurrentSession::new("s1", "w1"))
                .expect_err("symlinked session write should fail");
            assert!(error.to_string().contains("symlinked current session file"));
        }
        "write_non_regular_error" => {
            let error = write_current_session_file(&repo_root, &CurrentSession::new("s1", "w1"))
                .expect_err("socket session write should fail");
            assert!(
                error
                    .to_string()
                    .contains("current session file is not a regular file")
            );
        }
        other => panic!("unknown current session child case `{other}`"),
    }
}

#[test]
fn current_session_file_prefers_stateful_session_id_over_codex_aliases() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-current-session-generic-env-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    write_current_session_file(&temp_root, &CurrentSession::new("legacy-session", "w1"))
        .expect("legacy current session should write");
    write_current_session_file_for_session(
        &temp_root,
        "session-child",
        &CurrentSession::new("session-child", "w1"),
    )
    .expect("generic session-bound current session should write");
    write_current_session_file_for_session(
        &temp_root,
        "thread-child",
        &CurrentSession::new("thread-child", "w1"),
    )
    .expect("codex thread-bound current session should write");

    let output = Command::new(std::env::current_exe().expect("current test binary path"))
        .arg("current_session_file_child_probe")
        .arg("--ignored")
        .arg("--exact")
        .arg("--nocapture")
        .env_clear()
        .env(CURRENT_SESSION_CHILD_CASE, "read_uses_stateful_session_id")
        .env(CURRENT_SESSION_CHILD_ROOT, &temp_root)
        .env("STATEFUL_SESSION_ID", "session-child")
        .env("STATEFUL_CODEX_RUN_ID", "root-session")
        .env("CODEX_THREAD_ID", "thread-child")
        .output()
        .expect("current session child test should run");

    assert!(
        output.status.success(),
        "current session child failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn current_session_file_ignores_codex_aliases_without_stateful_session_id() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-current-session-ignore-codex-env-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    write_current_session_file(&temp_root, &CurrentSession::new("legacy-session", "w1"))
        .expect("legacy current session should write");
    write_current_session_file_for_session(
        &temp_root,
        "legacy-session",
        &CurrentSession::new("legacy-session", "w1"),
    )
    .expect("matching session-bound current session should write");

    let output = Command::new(std::env::current_exe().expect("current test binary path"))
        .arg("current_session_file_child_probe")
        .arg("--ignored")
        .arg("--exact")
        .arg("--nocapture")
        .env_clear()
        .env(CURRENT_SESSION_CHILD_CASE, "read_uses_legacy_session")
        .env(CURRENT_SESSION_CHILD_ROOT, &temp_root)
        .env("STATEFUL_CODEX_RUN_ID", "root-session")
        .env("CODEX_THREAD_ID", "thread-child")
        .output()
        .expect("current session child test should run");

    assert!(
        output.status.success(),
        "current session child failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn current_session_file_rejects_ambiguous_legacy_alias_without_stateful_session_id() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-current-session-ambiguous-legacy-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    write_current_session_file(&temp_root, &CurrentSession::new("legacy-session", "w1"))
        .expect("legacy current session should write");
    write_current_session_file_for_session(
        &temp_root,
        "legacy-session",
        &CurrentSession::new("legacy-session", "w1"),
    )
    .expect("matching session-bound current session should write");
    write_current_session_file_for_session(
        &temp_root,
        "other-session",
        &CurrentSession::new("other-session", "w1"),
    )
    .expect("second session-bound current session should write");

    let output = Command::new(std::env::current_exe().expect("current test binary path"))
        .arg("current_session_file_child_probe")
        .arg("--ignored")
        .arg("--exact")
        .arg("--nocapture")
        .env_clear()
        .env(
            CURRENT_SESSION_CHILD_CASE,
            "read_rejects_ambiguous_legacy_session",
        )
        .env(CURRENT_SESSION_CHILD_ROOT, &temp_root)
        .output()
        .expect("current session child test should run");

    assert!(
        output.status.success(),
        "current session child failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn current_session_file_rejects_unverified_legacy_alias_without_stateful_session_id() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-current-session-unverified-legacy-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    write_current_session_file(&temp_root, &CurrentSession::new("legacy-session", "w1"))
        .expect("legacy current session should write");
    write_current_session_file_for_session(
        &temp_root,
        "other-session",
        &CurrentSession::new("other-session", "w1"),
    )
    .expect("other session-bound current session should write");

    let output = Command::new(std::env::current_exe().expect("current test binary path"))
        .arg("current_session_file_child_probe")
        .arg("--ignored")
        .arg("--exact")
        .arg("--nocapture")
        .env_clear()
        .env(
            CURRENT_SESSION_CHILD_CASE,
            "read_rejects_unverified_legacy_session",
        )
        .env(CURRENT_SESSION_CHILD_ROOT, &temp_root)
        .output()
        .expect("current session child test should run");

    assert!(
        output.status.success(),
        "current session child failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn runtime_file_round_trips_server_discovery() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-runtime-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let runtime = ServerRuntime::new("http://127.0.0.1:43873", "secret-token", "w1", 42);
    write_runtime_file(&temp_root, &runtime).expect("runtime file should write");

    let discovered = discover_runtime(&temp_root).expect("runtime should be discoverable");

    assert_eq!(discovered.base_url, "http://127.0.0.1:43873");
    assert_eq!(discovered.token, "secret-token");
    assert_eq!(discovered.workspace_id, "w1");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn global_runtime_file_round_trips_server_discovery() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-global-runtime-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let repo_root = temp_root.join("repo");
    let paths = GlobalPaths::new(temp_root.join("home"));

    let runtime = ServerRuntime::new("http://127.0.0.1:43874", "global-token", "global-w", 43);
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let discovered =
        discover_runtime_with_global(&repo_root, &paths).expect("global runtime should discover");

    assert_eq!(discovered.base_url, "http://127.0.0.1:43874");
    assert_eq!(discovered.token, "global-token");
    assert_eq!(discovered.workspace_id, "global-w");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn runtime_discovery_keeps_local_runtime_compatibility_fallback() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-runtime-compat-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let repo_root = temp_root.join("repo");
    let paths = GlobalPaths::new(temp_root.join("home"));

    let runtime = ServerRuntime::new("http://127.0.0.1:43875", "repo-token", "repo-w", 44);
    write_runtime_file(&repo_root, &runtime).expect("repo runtime file should write");

    let discovered =
        discover_runtime_with_global(&repo_root, &paths).expect("repo runtime should discover");

    assert_eq!(discovered.base_url, "http://127.0.0.1:43875");
    assert_eq!(discovered.token, "repo-token");
    assert_eq!(discovered.workspace_id, "repo-w");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn cli_current_uses_local_runtime_when_global_paths_are_unavailable() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-runtime-no-home-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    fs::create_dir_all(temp_root.join(".git")).expect("git marker should write");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let _request = read_http_request_without_body(&mut stream);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\n{\"status\":\"ok\"}")
            .expect("response should write");
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    write_runtime_file(&temp_root, &runtime).expect("runtime file should write");

    let output = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .arg("current")
        .current_dir(&temp_root)
        .env_clear()
        .output()
        .expect("stateful current should run");

    assert!(
        output.status.success(),
        "stateful current failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"status\":\"ok\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[cfg(unix)]
#[test]
fn runtime_files_are_owner_read_write_only() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-runtime-mode-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let paths = GlobalPaths::new(temp_root.join("home"));
    let runtime = ServerRuntime::new("http://127.0.0.1:43875", "secret-token", "w1", 44);

    write_runtime_file(&temp_root, &runtime).expect("repo runtime file should write");
    write_global_runtime_file(&paths, &runtime).expect("global runtime file should write");

    let repo_mode = fs::metadata(temp_root.join(".stateful_core/runtime/server.json"))
        .expect("repo runtime metadata should read")
        .permissions()
        .mode()
        & 0o777;
    let global_mode = fs::metadata(&paths.server_json)
        .expect("global runtime metadata should read")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(repo_mode, 0o600);
    assert_eq!(global_mode, 0o600);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[cfg(unix)]
#[test]
fn current_session_files_are_owner_read_write_only() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-current-session-mode-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    write_current_session_file(&temp_root, &CurrentSession::new("legacy", "w1"))
        .expect("legacy session file should write");
    write_current_session_file_for_session(
        &temp_root,
        "session-1",
        &CurrentSession::new("session-1", "w1"),
    )
    .expect("session-bound session file should write");

    let legacy_mode = fs::metadata(temp_root.join(".stateful_core/runtime/session.json"))
        .expect("legacy session metadata should read")
        .permissions()
        .mode()
        & 0o777;
    let session_mode = fs::metadata(
        temp_root
            .join(".stateful_core/runtime/sessions")
            .join("session-1.json"),
    )
    .expect("session-bound metadata should read")
    .permissions()
    .mode()
        & 0o777;
    assert_eq!(legacy_mode, 0o600);
    assert_eq!(session_mode, 0o600);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn remote_runtime_with_pid_zero_accepts_matching_identity_capabilities() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        assert!(request.contains("GET /v1/runtime/identity HTTP/1.1"));
        write_http_response(
            &mut stream,
            r#"{"status":"ok","pid":9876,"protocol_version":"stateful.v1","capabilities":["authorize.write_directory"]}"#,
        );
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "shared", 0);

    assert!(stateful_cli::runtime_has_required_identity(&runtime));
}

#[test]
fn runtime_identity_matches_pid_requires_exact_pid_for_pid_zero_runtime() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        assert!(request.contains("GET /v1/runtime/identity HTTP/1.1"));
        write_http_response(
            &mut stream,
            r#"{"status":"ok","pid":9876,"protocol_version":"stateful.v1","capabilities":["authorize.write_directory"]}"#,
        );
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "shared", 0);

    assert!(
        !stateful_cli::runtime_identity_matches_pid(&runtime)
            .expect("identity check should succeed")
    );
}

#[test]
fn runtime_has_required_identity_rejects_mismatched_nonzero_pid() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        assert!(request.contains("GET /v1/runtime/identity HTTP/1.1"));
        write_http_response(
            &mut stream,
            r#"{"status":"ok","pid":9876,"protocol_version":"stateful.v1","capabilities":["authorize.write_directory"]}"#,
        );
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "shared", 42);

    assert!(!stateful_cli::runtime_has_required_identity(&runtime));
}

#[test]
fn cli_current_rejects_env_runtime_without_required_capabilities() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-runtime-env-old-server-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        assert!(request.contains("GET /v1/runtime/identity HTTP/1.1"));
        write_http_response(
            &mut stream,
            r#"{"status":"ok","pid":77,"protocol_version":"stateful.v1"}"#,
        );
    });

    let output = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .arg("current")
        .current_dir(&temp_root)
        .env_clear()
        .env("STATEFUL_SERVER_URL", format!("http://{addr}"))
        .env("STATEFUL_SERVER_TOKEN", "secret-token")
        .output()
        .expect("stateful current should run");

    assert!(
        !output.status.success(),
        "old env runtime should fail capability validation"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("does not support required runtime capabilities"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn cli_current_accepts_env_runtime_with_required_capabilities() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-runtime-env-capable-server-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    thread::spawn(move || {
        let (mut stream, _) = listener
            .accept()
            .expect("identity connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        assert!(request.contains("GET /v1/runtime/identity HTTP/1.1"));
        write_http_response(
            &mut stream,
            r#"{"status":"ok","pid":77,"protocol_version":"stateful.v1","capabilities":["authorize.write_directory"]}"#,
        );

        let (mut stream, _) = listener.accept().expect("current connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        assert!(request.contains("GET /v1/current HTTP/1.1"));
        write_http_response(&mut stream, r#"{"status":"ok","current":{}}"#);
    });

    let output = Command::new(env!("CARGO_BIN_EXE_stateful"))
        .arg("current")
        .current_dir(&temp_root)
        .env_clear()
        .env("STATEFUL_SERVER_URL", format!("http://{addr}"))
        .env("STATEFUL_SERVER_TOKEN", "secret-token")
        .output()
        .expect("stateful current should run");

    assert!(
        output.status.success(),
        "capable env runtime should work: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"status\":\"ok\""));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn current_session_file_round_trips_for_mcp_enrichment() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-current-session-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    run_current_session_child(&temp_root, "roundtrip_no_codex_env");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[cfg(unix)]
#[test]
fn current_session_file_refuses_symlink_before_reading() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-current-session-symlink-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(temp_root.join(".stateful_core/runtime"))
        .expect("runtime dir should be creatable");
    let victim = temp_root.join("victim-session.json");
    fs::write(&victim, r#"{"session_id":"s1","workspace_id":"w1"}"#)
        .expect("victim session should write");
    std::os::unix::fs::symlink(
        &victim,
        temp_root.join(".stateful_core/runtime/session.json"),
    )
    .expect("current session symlink should create");

    run_current_session_child(&temp_root, "read_symlink_error");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[cfg(unix)]
#[test]
fn current_session_file_refuses_symlink_before_writing() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-current-session-write-symlink-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(temp_root.join(".stateful_core/runtime"))
        .expect("runtime dir should be creatable");
    let victim = temp_root.join("victim-session.json");
    fs::write(&victim, "victim\n").expect("victim should write");
    std::os::unix::fs::symlink(
        &victim,
        temp_root.join(".stateful_core/runtime/session.json"),
    )
    .expect("current session symlink should create");

    run_current_session_child(&temp_root, "write_symlink_error");
    assert_eq!(
        fs::read_to_string(&victim).expect("victim should read"),
        "victim\n"
    );

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[cfg(unix)]
#[test]
fn current_session_file_refuses_non_regular_file_before_writing() {
    let temp_root = std::env::temp_dir().join(format!("scws-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(temp_root.join(".stateful_core/runtime"))
        .expect("runtime dir should be creatable");
    let session_path = temp_root.join(".stateful_core/runtime/session.json");
    let listener =
        std::os::unix::net::UnixListener::bind(&session_path).expect("session socket should bind");

    run_current_session_child(&temp_root, "write_non_regular_error");

    drop(listener);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn session_file_is_isolated_from_legacy_current_session() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-session-file-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    write_current_session_file(&temp_root, &CurrentSession::new("s-legacy", "w1"))
        .expect("legacy current session should write");
    write_current_session_file_for_session(
        &temp_root,
        "s-run-a",
        &CurrentSession::new("s-run-a", "w1"),
    )
    .expect("session-bound current session should write");

    let session =
        read_current_session_file_for_session(&temp_root, "s-run-a").expect("run session reads");

    assert_eq!(session.session_id, "s-run-a");
    assert_eq!(session.workspace_id, "w1");

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn session_file_refuses_to_rebind_to_different_stateful_session() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-session-file-rebind-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    write_current_session_file_for_session(
        &temp_root,
        "s-run-a",
        &CurrentSession::new("s-run-a", "w1"),
    )
    .expect("session-bound current session should write");

    let error = write_current_session_file_for_session(
        &temp_root,
        "s-run-a",
        &CurrentSession::new("s-run-a", "w2"),
    )
    .expect_err("same session file should not rebind to a different Stateful session");

    assert!(error.to_string().contains("already bound"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn state_db_path_uses_local_runtime_directory() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-runtime-db-test-{}", std::process::id()));

    assert_eq!(
        state_db_path(&temp_root),
        temp_root.join(".stateful_core").join("state.db")
    );
}

#[test]
fn global_state_db_path_uses_user_level_state_db() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-global-runtime-db-test-{}",
        std::process::id()
    ));
    let paths = GlobalPaths::new(temp_root.join("home"));

    assert_eq!(global_state_db_path(&paths), paths.state_db);
}

#[test]
fn post_json_sends_bearer_token_and_payload() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\n{\"status\":\"ok\"}")
            .expect("response should write");
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let response = post_json(
        &runtime,
        "/v1/intent/declare",
        &serde_json::json!({
            "session_id": "s1",
            "workspace_id": "w1",
            "purpose": "Fix auth validation behavior.",
            "files_planned": ["src/auth.ts"]
        }),
    )
    .expect("post should succeed");

    assert_eq!(response.status_code, 200);

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/intent/declare HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));
    assert!(request.contains("\"purpose\":\"Fix auth validation behavior.\""));
    assert!(request.contains("\"files_planned\":[\"src/auth.ts\"]"));
}

#[test]
fn get_json_sends_bearer_token_without_body() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request_without_body(&mut stream);
        tx.send(request).expect("request should send to test");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\n{\"status\":\"ok\"}")
            .expect("response should write");
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);
    let response = get_json(&runtime, "/v1/current").expect("get should succeed");

    assert_eq!(response.status_code, 200);

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("GET /v1/current HTTP/1.1"));
    assert!(request.contains("Authorization: Bearer secret-token"));
    assert!(!request.contains("Content-Length:"));
}

#[test]
fn declare_intent_via_http_posts_expected_payload() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\n{\"status\":\"ok\"}")
            .expect("response should write");
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);

    let before_request = OffsetDateTime::now_utc();
    declare_intent_via_http(
        &runtime,
        IntentDeclareArgs {
            session_id: "s1".to_string(),
            workspace_id: "w1".to_string(),
            purpose: "Fix auth validation behavior.".to_string(),
            files_planned: vec!["src/auth.ts".to_string()],
            identity: None,
        },
    )
    .expect("intent declaration should post");

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/intent/declare HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v1");
    assert!(
        body["request_id"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    let observed_at = body["observed_at"]
        .as_str()
        .expect("observed_at should be a string");
    assert_ne!(
        observed_at, runtime.started_at,
        "observed_at should describe the request, not the runtime start"
    );
    let observed_at = OffsetDateTime::parse(observed_at, &Rfc3339)
        .expect("observed_at should be an RFC3339 timestamp");
    let after_request = OffsetDateTime::now_utc();
    assert!(observed_at >= before_request);
    assert!(observed_at <= after_request);
    assert_eq!(body["session"]["session_id"], "s1");
    assert_eq!(body["session"]["actor_id"], "stateful-cli:42");
    assert_eq!(body["session"]["actor_type"], "agent");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(body["workspace"]["repo_id"], "");
    assert_eq!(body["workspace"]["worktree_id"], "");
    assert_eq!(body["workspace"]["root"], "");
    assert_eq!(body["workspace"]["branch"], "");
    assert_eq!(body["source"]["kind"], "cli");
    assert_eq!(body["source"]["event"], "intent_declare");
    assert_eq!(body["source"]["source_ref"], "stateful-cli");
    assert_eq!(
        body["payload"],
        serde_json::json!({
            "purpose": "Fix auth validation behavior.",
            "files_planned": ["src/auth.ts"]
        })
    );
    assert!(body.get("files_planned").is_none());
}

#[test]
fn claim_intent_via_http_posts_expected_payload() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\n{\"status\":\"ok\"}")
            .expect("response should write");
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);

    claim_intent_via_http(
        &runtime,
        IntentClaimArgs {
            session_id: "s1".to_string(),
            workspace_id: "w1".to_string(),
            wait_id: "wait-1".to_string(),
            identity: None,
        },
    )
    .expect("intent claim should post");

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/intent/claim HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v1");
    assert_eq!(body["session"]["session_id"], "s1");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(body["source"]["kind"], "cli");
    assert_eq!(body["source"]["event"], "intent_claim");
    assert_eq!(body["source"]["source_ref"], "stateful-cli");
    assert_eq!(
        body["payload"],
        serde_json::json!({
            "wait_id": "wait-1"
        })
    );
    assert!(body.get("wait_id").is_none());
}

#[test]
fn request_intent_via_http_posts_expected_payload() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\n{\"status\":\"ok\"}")
            .expect("response should write");
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);

    let response = request_intent_via_http(
        &runtime,
        IntentRequestArgs {
            session_id: "s1".to_string(),
            workspace_id: "w1".to_string(),
            request_id: "request-1".to_string(),
            action: "write_file".to_string(),
            path: "src/auth.ts".to_string(),
            purpose: "Queue auth file changes.".to_string(),
            identity: None,
        },
    )
    .expect("intent request should post");
    assert_eq!(response.status_code, 200);
    assert_eq!(response.body, "{\"status\":\"ok\"}");

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/intent/request HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v1");
    assert_eq!(body["session"]["session_id"], "s1");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(body["source"]["kind"], "cli");
    assert_eq!(body["source"]["event"], "intent_request");
    assert_eq!(body["source"]["source_ref"], "stateful-cli");
    assert_eq!(
        body["payload"],
        serde_json::json!({
            "request_id": "request-1",
            "action": "write_file",
            "path": "src/auth.ts",
            "purpose": "Queue auth file changes."
        })
    );
    assert!(body.get("request_id").is_some());
}

#[test]
fn cancel_intent_via_http_posts_expected_payload() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr should load");
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection should arrive");
        let request = read_http_request(&mut stream);
        tx.send(request).expect("request should send to test");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\n{\"status\":\"ok\"}")
            .expect("response should write");
    });

    let runtime = ServerRuntime::new(format!("http://{addr}"), "secret-token", "w1", 42);

    cancel_intent_via_http(
        &runtime,
        IntentCancelArgs {
            session_id: "s1".to_string(),
            workspace_id: "w1".to_string(),
            request_id: "request-1".to_string(),
            identity: None,
        },
    )
    .expect("intent cancel should post");

    let request = rx.recv().expect("captured request should arrive");
    assert!(request.contains("POST /v1/intent/cancel HTTP/1.1"));
    let body = request_json_body(&request);
    assert_eq!(body["protocol_version"], "stateful.v1");
    assert_eq!(body["session"]["session_id"], "s1");
    assert_eq!(body["workspace"]["workspace_id"], "w1");
    assert_eq!(body["source"]["kind"], "cli");
    assert_eq!(body["source"]["event"], "intent_cancel");
    assert_eq!(body["source"]["source_ref"], "stateful-cli");
    assert_eq!(
        body["payload"],
        serde_json::json!({
            "request_id": "request-1"
        })
    );
}

fn request_json_body(request: &str) -> serde_json::Value {
    let (_, body) = request
        .split_once("\r\n\r\n")
        .expect("request should contain a body separator");
    serde_json::from_str(body).expect("request body should be json")
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

fn write_http_response(stream: &mut std::net::TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .expect("response should write");
}

fn read_http_request_without_body(stream: &mut std::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut byte = [0_u8; 1];
    while !buffer.ends_with(b"\r\n\r\n") {
        stream
            .read_exact(&mut byte)
            .expect("request header byte should read");
        buffer.push(byte[0]);
    }

    String::from_utf8(buffer).expect("request should be utf8")
}
