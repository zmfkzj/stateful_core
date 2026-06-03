use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use stateful_cli::{GlobalPaths, ServerRuntime, ensure_server_with};

#[test]
fn ensure_server_returns_existing_runtime_when_health_is_ok() {
    let home = temp_home("stateful-server-existing");
    let paths = GlobalPaths::new(&home);
    let runtime = ServerRuntime::new("http://127.0.0.1:43873", "token", "w1", 123);
    stateful_cli::write_global_runtime_file(&paths, &runtime).expect("runtime should write");
    let starts = Arc::new(AtomicUsize::new(0));
    let starts_for_closure = starts.clone();

    let discovered = ensure_server_with(
        &paths,
        |_| true,
        || {
            starts_for_closure.fetch_add(1, Ordering::SeqCst);
            Ok(runtime.clone())
        },
    )
    .expect("server should ensure");

    assert_eq!(discovered.pid, 123);
    assert_eq!(starts.load(Ordering::SeqCst), 0);
}

#[test]
fn ensure_server_starts_when_runtime_is_missing() {
    let home = temp_home("stateful-server-missing");
    let paths = GlobalPaths::new(&home);
    let starts = Arc::new(AtomicUsize::new(0));
    let starts_for_closure = starts.clone();

    let discovered = ensure_server_with(
        &paths,
        |_| true,
        || {
            starts_for_closure.fetch_add(1, Ordering::SeqCst);
            Ok(ServerRuntime::new(
                "http://127.0.0.1:43873",
                "token",
                "w1",
                456,
            ))
        },
    )
    .expect("server should start");

    assert_eq!(discovered.pid, 456);
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert!(paths.server_json.is_file());
}

#[test]
fn ensure_server_does_not_steal_existing_start_lock() {
    let home = temp_home("stateful-server-locked");
    let paths = GlobalPaths::new(&home);
    fs::create_dir_all(&paths.runtime_dir).expect("runtime dir should be creatable");
    fs::write(&paths.server_lock, "other-process").expect("lock should be writable");
    let starts = Arc::new(AtomicUsize::new(0));
    let starts_for_closure = starts.clone();

    let error = ensure_server_with(
        &paths,
        |_| true,
        || {
            starts_for_closure.fetch_add(1, Ordering::SeqCst);
            Ok(ServerRuntime::new(
                "http://127.0.0.1:43873",
                "token",
                "w1",
                789,
            ))
        },
    )
    .expect_err("server should not start while another start lock exists");

    assert!(
        error.to_string().contains("start lock"),
        "unexpected error: {error}"
    );
    assert_eq!(starts.load(Ordering::SeqCst), 0);
    assert!(paths.server_lock.is_file());
    assert_eq!(
        fs::read_to_string(&paths.server_lock).expect("lock should remain readable"),
        "other-process"
    );
}

fn temp_home(name: &str) -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    if home.exists() {
        fs::remove_dir_all(&home).expect("old home should be removable");
    }
    home
}
