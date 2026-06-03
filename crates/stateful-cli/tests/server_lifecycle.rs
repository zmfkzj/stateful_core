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

fn temp_home(name: &str) -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    if home.exists() {
        fs::remove_dir_all(&home).expect("old home should be removable");
    }
    home
}
