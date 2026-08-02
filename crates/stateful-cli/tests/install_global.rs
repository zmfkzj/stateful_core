use stateful_cli::{
    CodexInstallOptions, GlobalPaths, OmpInstallOptions, apply_codex_install, plan_codex_install,
    plan_omp_install,
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
