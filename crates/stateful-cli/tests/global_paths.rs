use stateful_cli::GlobalPaths;

#[test]
fn global_paths_are_rooted_under_stateful_home() {
    let home = std::env::temp_dir().join(format!("stateful-home-{}", std::process::id()));
    let paths = GlobalPaths::new(&home);

    assert_eq!(paths.home, home);
    assert_eq!(paths.config_yml, paths.home.join("config.yml"));
    assert_eq!(paths.state_db, paths.home.join("state.db"));
    assert_eq!(paths.runtime_dir, paths.home.join("runtime"));
    assert_eq!(paths.server_json, paths.home.join("runtime/server.json"));
    assert_eq!(paths.server_lock, paths.home.join("runtime/server.lock"));
    assert_eq!(paths.server_log, paths.home.join("runtime/server.log"));
    assert_eq!(paths.repos_dir, paths.home.join("repos"));
}
