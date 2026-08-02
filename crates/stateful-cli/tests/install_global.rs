use stateful_cli::{
    CodexInstallOptions, GlobalPaths, InstallOptions, OmpInstallOptions, RepoRegistry,
    apply_codex_install, apply_global_install, plan_codex_install, plan_omp_install,
};

#[test]
fn codex_plan_contains_only_global_files_and_config() {
    let temp = tempfile::tempdir().expect("temp directory should create");
    let paths = GlobalPaths::new(temp.path().join("stateful"));
    let config = temp.path().join("codex/config.toml");
    let plan = plan_codex_install(&CodexInstallOptions {
        yes: false,
        paths: paths.clone(),
        codex_config_path: config.clone(),
        binary_path: "/opt/stateful/bin/stateful".to_string(),
    })
    .expect("plan should succeed");

    assert!(plan.files.contains(&paths.home));
    assert!(plan.files.contains(&config));
    assert_eq!(plan.files.len(), 6);
}

#[test]
fn codex_hook_commands_exec_the_stateful_binary() {
    let temp = tempfile::tempdir().expect("temp directory should create");
    let paths = GlobalPaths::new(temp.path().join("stateful"));
    let config = temp.path().join("codex/config.toml");
    apply_codex_install(CodexInstallOptions {
        yes: true,
        paths,
        codex_config_path: config.clone(),
        binary_path: "/opt/stateful bin/stateful".to_string(),
    })
    .expect("install should succeed");

    let contents = std::fs::read_to_string(config).expect("config should read");
    assert_eq!(
        contents
            .matches("command = \"exec '/opt/stateful bin/stateful' hook codex")
            .count(),
        5
    );
}

#[test]
fn global_install_migrates_removed_writer_policy() {
    let temp = tempfile::tempdir().expect("temp directory should create");
    let paths = GlobalPaths::new(temp.path().join("stateful"));
    std::fs::create_dir_all(&paths.home).expect("stateful home should create");
    std::fs::write(
        &paths.config_yml,
        "repos:
  - repo_id: repo-1
    root: /tmp/repo
    enabled: true
    enabled_at: '1'
    policy_config_path: /tmp/repo/.stateful/config.yml
    allowed_tools: [write]
    unclassified_tools: [unknown]
",
    )
    .expect("legacy registry should write");

    apply_global_install(InstallOptions {
        yes: true,
        paths: paths.clone(),
    })
    .expect("global install should migrate the registry");

    let registry = RepoRegistry::load(&paths).expect("migrated registry should load");
    assert_eq!(registry.repos.len(), 1);
    assert_eq!(registry.repos[0].repo_id, "repo-1");
    let config = std::fs::read_to_string(&paths.config_yml).expect("registry should read");
    assert!(!config.contains("allowed_tools"));
    assert!(!config.contains("unclassified_tools"));
    assert!(paths.state_db.is_file());
}

#[test]
fn global_install_rejects_unknown_existing_database() {
    let temp = tempfile::tempdir().expect("temp directory should create");
    let paths = GlobalPaths::new(temp.path().join("stateful"));
    std::fs::create_dir_all(&paths.home).expect("stateful home should create");
    std::fs::write(&paths.state_db, "not a sqlite database")
        .expect("invalid database should write");

    let error = apply_global_install(InstallOptions { yes: true, paths })
        .expect_err("global install should inspect an existing database");

    assert!(
        error
            .to_string()
            .contains("failed to initialize state database")
    );
}

#[test]
fn omp_plan_contains_config_and_v2_extension() {
    let temp = tempfile::tempdir().expect("temp directory should create");
    let paths = GlobalPaths::new(temp.path().join("stateful"));
    let agent_dir = temp.path().join("omp-agent");
    let config = agent_dir.join("config.yml");
    let extension = agent_dir.join("extensions/stateful-omp-extension.js");
    let plan = plan_omp_install(&OmpInstallOptions {
        yes: false,
        paths,
        binary_path: "/opt/stateful/bin/stateful".to_string(),
        project_config_path: Some(config.clone()),
        omp_agent_dir: Some(agent_dir),
        update: false,
    })
    .expect("plan should succeed");

    assert!(plan.files.contains(&config));
    assert!(plan.files.contains(&extension));
    assert_eq!(plan.files.len(), 7);
}
