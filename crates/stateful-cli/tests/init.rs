use std::fs;

use stateful_cli::{GlobalPaths, doctor_report, enable_repo, install_repo_local};

#[test]
fn install_repo_local_writes_codex_hooks_and_stateful_config() {
    let temp_root = std::env::temp_dir().join(format!("stateful-init-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    install_repo_local(&temp_root, "target/debug/stateful").expect("repo install should succeed");

    let hooks =
        fs::read_to_string(temp_root.join(".codex/hooks.json")).expect("hooks.json should exist");
    let parsed: serde_json::Value =
        serde_json::from_str(&hooks).expect("hooks should be valid json");
    assert_eq!(parsed["stateful_core_owned"], true);
    assert!(hooks.contains("\"SessionStart\""));
    assert!(hooks.contains("\"PreToolUse\""));
    assert!(hooks.contains("\"PostToolUse\""));
    assert!(hooks.contains("\"Stop\""));
    assert!(hooks.contains("target/debug/stateful"));
    assert!(hooks.contains("hook pre-tool-use"));
    assert_eq!(
        parsed["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        "\"$(git rev-parse --show-toplevel)/target/debug/stateful\" hook pre-tool-use"
    );

    let config =
        fs::read_to_string(temp_root.join(".stateful/config.yml")).expect("config should exist");
    assert!(config.contains("protocol_version: stateful.v1"));
    assert!(config.contains("default_write_policy: deny"));

    let codex_config = fs::read_to_string(temp_root.join(".codex/config.toml"))
        .expect("codex config should exist");
    assert!(codex_config.contains("# stateful-core-owned"));
    assert!(codex_config.contains("hooks = true"));
    assert!(codex_config.contains("[[hooks.PreToolUse]]"));
    assert!(codex_config.contains("hook pre-tool-use"));

    let command_policy_skill = fs::read_to_string(
        temp_root.join(".codex/skills/stateful-command-policy/SKILL.md"),
    )
    .expect("stateful command policy skill should exist");
    assert!(command_policy_skill.contains("name: stateful-command-policy"));
    assert!(command_policy_skill.contains("Use when running shell commands"));
    assert!(command_policy_skill.contains("stateful intent declare"));

    let validation = fs::read_to_string(temp_root.join(".stateful/validation.yml"))
        .expect("validation config should exist");
    assert!(validation.contains("profiles:"));
    assert!(validation.contains("profile_id: cargo-test"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn install_repo_local_refuses_existing_non_stateful_codex_config() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-init-existing-config-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(temp_root.join(".codex")).expect("codex dir should be creatable");
    let config_path = temp_root.join(".codex/config.toml");
    fs::write(&config_path, "[mcp_servers.other]\ncommand = \"other\"\n")
        .expect("existing codex config should be writable");

    let error = install_repo_local(&temp_root, "target/debug/stateful")
        .expect_err("repo install should refuse existing non-stateful config");

    assert!(error.to_string().contains("would overwrite existing Codex config"));
    let config = fs::read_to_string(config_path).expect("existing config should remain readable");
    assert_eq!(config, "[mcp_servers.other]\ncommand = \"other\"\n");
    assert!(!temp_root.join(".stateful/config.yml").exists());
    assert!(!temp_root.join(".stateful/validation.yml").exists());

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn install_repo_local_output_can_be_enabled_as_repo_local_codex() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-init-enable-repo-local-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(temp_root.join(".git")).expect("git dir should be creatable");

    install_repo_local(&temp_root, "/opt/stateful/bin/stateful")
        .expect("repo install should succeed");
    let paths = GlobalPaths::new(temp_root.join("home"));

    let entry = enable_repo(&paths, &temp_root, true).expect("repo should enable");

    assert!(entry.enabled);
    let hooks =
        fs::read_to_string(temp_root.join(".codex/hooks.json")).expect("hooks should exist");
    let hooks: serde_json::Value = serde_json::from_str(&hooks).expect("hooks should be json");
    assert_eq!(hooks["stateful_core_owned"], true);
    let config =
        fs::read_to_string(temp_root.join(".codex/config.toml")).expect("config should exist");
    assert!(config.contains("# stateful-core-owned"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn install_repo_local_preserves_absolute_binary_paths() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-init-absolute-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    install_repo_local(&temp_root, "/opt/stateful/bin/stateful")
        .expect("repo install should succeed");

    let hooks =
        fs::read_to_string(temp_root.join(".codex/hooks.json")).expect("hooks.json should exist");
    let parsed: serde_json::Value =
        serde_json::from_str(&hooks).expect("hooks should be valid json");
    assert_eq!(
        parsed["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        "\"/opt/stateful/bin/stateful\" hook pre-tool-use"
    );

    let config =
        fs::read_to_string(temp_root.join(".codex/config.toml")).expect("config should exist");
    assert!(config.contains("command = \"/opt/stateful/bin/stateful\""));
    assert!(config.contains("hook pre-tool-use"));

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn doctor_report_marks_repo_local_installation() {
    let temp_root =
        std::env::temp_dir().join(format!("stateful-doctor-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    install_repo_local(&temp_root, "target/debug/stateful").expect("repo install should succeed");

    let report = doctor_report(&temp_root);

    assert!(report.installed);
    assert!(report.hooks_json);
    assert!(report.config_yml);
    assert!(report.validation_yml);
    assert!(!report.runtime_server_json);
    assert!(!report.state_db);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}
