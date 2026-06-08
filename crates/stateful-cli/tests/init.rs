use std::{fs, path::Path};

use stateful_cli::{
    GlobalPaths, doctor_report, doctor_report_with_global, enable_repo, install_repo_local,
    install_repo_local_with_global_codex_config,
};

#[test]
fn install_repo_local_writes_config_toml_hooks_and_stateful_config() {
    let temp_root = std::env::temp_dir().join(format!("stateful-init-test-{}", std::process::id()));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");

    install_repo_local(&temp_root, "target/debug/stateful").expect("repo install should succeed");

    assert!(!temp_root.join(".codex/hooks.json").exists());

    let config =
        fs::read_to_string(temp_root.join(".stateful/config.yml")).expect("config should exist");
    assert!(config.contains("protocol_version: stateful.v1"));
    assert!(config.contains("default_write_policy: deny"));

    let codex_config = fs::read_to_string(temp_root.join(".codex/config.toml"))
        .expect("codex config should exist");
    assert!(codex_config.contains("# stateful-core-owned"));
    assert!(codex_config.contains("hooks = true"));
    assert!(codex_config.contains("[mcp_servers.stateful]"));
    assert!(codex_config.contains(
        "env_vars = [\"STATEFUL_CODEX_RUN_ID\", \"CODEX_THREAD_ID\", \"STATEFUL_SERVER_URL\", \"STATEFUL_SERVER_TOKEN\"]"
    ));
    assert!(codex_config.contains("[[hooks.PreToolUse]]"));
    assert!(codex_config.contains("$(git rev-parse --show-toplevel)/target/debug/stateful"));
    assert!(codex_config.contains("hook pre-tool-use"));

    let command_policy_skill =
        fs::read_to_string(temp_root.join(".codex/skills/stateful-command-policy/SKILL.md"))
            .expect("stateful command policy skill should exist");
    let source_command_policy_skill = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/stateful-command-policy/SKILL.md"),
    )
    .expect("source stateful command policy skill should exist");
    assert_eq!(command_policy_skill, source_command_policy_skill);
    assert!(command_policy_skill.contains("name: stateful-command-policy"));
    assert!(command_policy_skill.contains("Use before any Bash or shell command"));
    assert!(command_policy_skill.contains("state_intent_declare"));
    assert!(command_policy_skill.contains("Raw Bash is denied by stateful hooks"));
    assert!(
        command_policy_skill
            .contains("<absolute-stateful-binary> sandbox run --fs read-only --network disabled")
    );
    assert!(
        command_policy_skill.contains("<absolute-stateful-binary> sandbox run --fs write-targets")
    );
    assert!(
        command_policy_skill
            .contains("trusted absolute `stateful` binary installed in the hook configuration")
    );
    assert!(!command_policy_skill.contains("stateful intent declare"));
    assert!(!command_policy_skill.contains("state_bash_write"));
    assert!(!command_policy_skill.contains("state.bash.write"));
    assert!(!command_policy_skill.contains("top-level read-only sandbox metadata"));
    assert!(command_policy_skill.contains("MCP or native read tools"));
    assert!(command_policy_skill.contains("apply_patch"));
    assert!(command_policy_skill.contains("Edit"));
    assert!(!command_policy_skill.contains("state_file_write"));
    assert!(command_policy_skill.contains("Raw test commands"));
    assert!(command_policy_skill.contains("Examples assume `<absolute-stateful-binary>`"));
    assert!(command_policy_skill.contains(
        "<absolute-stateful-binary> intent declare --session-id <session> --workspace-id <workspace> --purpose \"<purpose inferred from the user or agent instruction>\" target/"
    ));
    assert!(command_policy_skill.contains("--create-target target/generated.txt"));
    assert!(!command_policy_skill.contains("--create-target docs/new.md"));
    assert!(!command_policy_skill.contains("--write-target README.md"));
    assert!(command_policy_skill.contains("--write-dir target"));
    assert!(command_policy_skill.contains("`stateful sandbox run` targets must be repo-relative"));
    assert!(command_policy_skill.contains("external-run request --purpose"));
    assert!(command_policy_skill.contains("copy-paste `external-run approve <id> --run`"));
    assert!(command_policy_skill.contains("whole external directories after user approval"));
    let former_local_home = String::from_utf8(vec![
        47, 85, 115, 101, 114, 115, 47, 97, 114, 116, 104, 117, 114,
    ])
    .expect("literal should be valid UTF-8");
    assert!(!command_policy_skill.contains(&former_local_home));
    assert!(command_policy_skill.contains("Raw read-only Bash is also denied"));
    assert!(command_policy_skill.contains("Use `stateful commit` / `stateful push`"));
    assert!(!command_policy_skill.contains("stateful validate cargo-test"));
    assert!(!command_policy_skill.contains("state_validation_run"));
    assert!(command_policy_skill.contains("`/dev/null` is writable inside the sandbox"));
    assert!(command_policy_skill.contains("macOS-first"));
    assert!(command_policy_skill.contains("Linux bubblewrap support"));

    assert!(!temp_root.join(".stateful/validation.yml").exists());

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn install_repo_local_omits_hooks_when_global_stateful_hooks_are_installed() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-init-global-hooks-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(&temp_root).expect("temp root should be creatable");
    let global_config = temp_root.join("global-codex-config.toml");
    fs::write(
        &global_config,
        r#"# stateful-core-global-install
[features]
hooks = true

[mcp_servers.stateful]
command = "/opt/stateful/bin/stateful"
args = ["mcp", "serve"]

[[hooks.UserPromptSubmit]]

[[hooks.UserPromptSubmit.hooks]]
type = "command"
command = "'/opt/stateful/bin/stateful' hook user-prompt-submit"
# /stateful-core-global-install
"#,
    )
    .expect("global config should be writable");

    install_repo_local_with_global_codex_config(
        &temp_root,
        "target/debug/stateful",
        Some(global_config.as_path()),
    )
    .expect("repo install should succeed");

    let codex_config = fs::read_to_string(temp_root.join(".codex/config.toml"))
        .expect("codex config should exist");
    assert!(codex_config.contains("# stateful-core-owned"));
    assert!(codex_config.contains("[mcp_servers.stateful]"));
    assert!(!codex_config.contains("hooks = true"));
    assert!(!codex_config.contains("[[hooks.UserPromptSubmit]]"));
    assert!(!codex_config.contains("[[hooks.PreToolUse]]"));
    assert!(!codex_config.contains("hook user-prompt-submit"));
    assert!(!codex_config.contains("hook pre-tool-use"));
    assert!(
        temp_root
            .join(".codex/skills/stateful-command-policy/SKILL.md")
            .is_file()
    );
    assert!(temp_root.join(".stateful/config.yml").is_file());
    assert!(!temp_root.join(".stateful/validation.yml").exists());

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

    assert!(
        error
            .to_string()
            .contains("would overwrite existing Codex config")
    );
    let config = fs::read_to_string(config_path).expect("existing config should remain readable");
    assert_eq!(config, "[mcp_servers.other]\ncommand = \"other\"\n");
    assert!(!temp_root.join(".stateful/config.yml").exists());
    assert!(!temp_root.join(".stateful/validation.yml").exists());

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn install_repo_local_refuses_existing_non_stateful_hooks_json() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-init-existing-hooks-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(temp_root.join(".codex")).expect("codex dir should be creatable");
    let hooks_path = temp_root.join(".codex/hooks.json");
    let hooks = r#"{
  "hooks": {
    "PreToolUse": [{
      "hooks": [{
        "type": "command",
        "command": "custom-tool hook pre-tool-use"
      }]
    }]
  }
}
"#;
    fs::write(&hooks_path, hooks).expect("existing hooks should be writable");

    let error = install_repo_local(&temp_root, "target/debug/stateful")
        .expect_err("repo install should refuse existing non-stateful hooks");

    assert!(
        error
            .to_string()
            .contains("would overwrite existing Codex config")
    );
    let saved_hooks =
        fs::read_to_string(hooks_path).expect("existing hooks should remain readable");
    assert_eq!(saved_hooks, hooks);
    assert!(!temp_root.join(".codex/config.toml").exists());
    assert!(!temp_root.join(".stateful/config.yml").exists());
    assert!(!temp_root.join(".stateful/validation.yml").exists());

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn install_repo_local_refuses_unmarked_stateful_mcp_config_without_legacy_hooks() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-init-unmarked-stateful-config-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(temp_root.join(".codex")).expect("codex dir should be creatable");
    let config_path = temp_root.join(".codex/config.toml");
    let config = r#"[mcp_servers.stateful]
command = "stateful"
args = ["mcp", "serve"]
startup_timeout_sec = 20
"#;
    fs::write(&config_path, config).expect("existing codex config should be writable");

    let error = install_repo_local(&temp_root, "target/debug/stateful")
        .expect_err("repo install should refuse unmarked stateful config without legacy hooks");

    assert!(
        error
            .to_string()
            .contains("would overwrite existing Codex config")
    );
    let saved_config =
        fs::read_to_string(config_path).expect("existing config should remain readable");
    assert_eq!(saved_config, config);
    assert!(!temp_root.join(".codex/hooks.json").exists());
    assert!(!temp_root.join(".stateful/config.yml").exists());
    assert!(!temp_root.join(".stateful/validation.yml").exists());

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn install_repo_local_replaces_unmarked_legacy_stateful_codex_files() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-init-legacy-codex-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(temp_root.join(".codex")).expect("codex dir should be creatable");
    fs::write(
        temp_root.join(".codex/hooks.json"),
        r#"{
  "hooks": {
    "PostToolUse": [{
      "hooks": [{
        "command": "stateful hook post-tool-use",
        "statusMessage": "Recording stateful activity",
        "type": "command"
      }],
      "matcher": "Bash|apply_patch|Edit|Write|mcp__filesystem__.*"
    }],
    "PreToolUse": [{
      "hooks": [{
        "command": "stateful hook pre-tool-use",
        "statusMessage": "Authorizing stateful tool use",
        "type": "command"
      }],
      "matcher": "Bash|apply_patch|Edit|Write|mcp__filesystem__.*"
    }],
    "SessionStart": [{
      "hooks": [{
        "command": "stateful hook session-start",
        "statusMessage": "Loading stateful current state",
        "type": "command"
      }],
      "matcher": "startup|resume|clear|compact"
    }],
    "Stop": [{
      "hooks": [{
        "command": "stateful hook stop",
        "statusMessage": "Finalizing stateful activity",
        "type": "command"
      }]
    }],
    "UserPromptSubmit": [{
      "hooks": [{
        "command": "stateful hook user-prompt-submit",
        "statusMessage": "Checking stateful intent context",
        "type": "command"
      }]
    }]
  }
}
"#,
    )
    .expect("legacy hooks should be writable");
    fs::write(
        temp_root.join(".codex/config.toml"),
        r#"[mcp_servers.stateful]
command = "stateful"
args = ["mcp", "serve"]
startup_timeout_sec = 20

[mcp_servers.stateful.tools.state_intent_declare]
approval_mode = "approve"

[mcp_servers.stateful.tools.state_conflicts_check]
approval_mode = "approve"
"#,
    )
    .expect("legacy config should be writable");

    install_repo_local(&temp_root, "target/debug/stateful").expect("repo install should succeed");

    assert!(!temp_root.join(".codex/hooks.json").exists());
    let codex_config = fs::read_to_string(temp_root.join(".codex/config.toml"))
        .expect("codex config should exist");
    assert!(codex_config.contains("# stateful-core-owned"));
    assert!(codex_config.contains("[[hooks.PreToolUse]]"));
    assert!(codex_config.contains("hook pre-tool-use"));
    assert!(temp_root.join(".stateful/config.yml").exists());
    assert!(!temp_root.join(".stateful/validation.yml").exists());

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn install_repo_local_removes_existing_stateful_owned_hooks_json() {
    let temp_root = std::env::temp_dir().join(format!(
        "stateful-init-old-hooks-test-{}",
        std::process::id()
    ));
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root).expect("old temp root should be removable");
    }
    fs::create_dir_all(temp_root.join(".codex")).expect("codex dir should be creatable");
    fs::write(
        temp_root.join(".codex/hooks.json"),
        r#"{
  "stateful_core_owned": true,
  "hooks": {
    "PreToolUse": [{
      "hooks": [{
        "type": "command",
        "command": "old-stateful hook pre-tool-use"
      }]
    }]
  }
}
"#,
    )
    .expect("old stateful-owned hooks should be writable");

    install_repo_local(&temp_root, "target/debug/stateful").expect("repo install should succeed");

    assert!(!temp_root.join(".codex/hooks.json").exists());
    let codex_config = fs::read_to_string(temp_root.join(".codex/config.toml"))
        .expect("codex config should exist");
    assert!(codex_config.contains("# stateful-core-owned"));
    assert!(codex_config.contains("[[hooks.PreToolUse]]"));

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
    assert!(!temp_root.join(".codex/hooks.json").exists());
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

    assert!(!temp_root.join(".codex/hooks.json").exists());

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
    fs::create_dir_all(temp_root.join(".git")).expect("git marker should write");
    install_repo_local(&temp_root, "target/debug/stateful").expect("repo install should succeed");

    let report = doctor_report(&temp_root);

    assert!(report.installed);
    assert!(!report.hooks_json);
    assert!(report.config_yml);
    assert!(!report.runtime_server_json);
    assert!(!report.state_db);

    fs::remove_dir_all(&temp_root).expect("temp root should be removable");
}

#[test]
fn doctor_report_includes_global_install_and_repo_enabled_status() {
    let repo = std::env::temp_dir().join(format!(
        "stateful-doctor-global-repo-{}",
        std::process::id()
    ));
    let home = std::env::temp_dir().join(format!(
        "stateful-doctor-global-home-{}",
        std::process::id()
    ));
    if repo.exists() {
        fs::remove_dir_all(&repo).expect("old repo should be removable");
    }
    if home.exists() {
        fs::remove_dir_all(&home).expect("old home should be removable");
    }
    fs::create_dir_all(repo.join(".git")).expect("git marker should write");
    let paths = stateful_cli::GlobalPaths::new(&home);
    stateful_cli::enable_repo(&paths, &repo, false).expect("repo should enable");

    let report = doctor_report_with_global(&repo, &paths);

    assert!(report.global_config_yml);
    assert!(report.repo_enabled);
    assert!(report.config_yml);
    fs::remove_dir_all(repo).expect("repo should remove");
    fs::remove_dir_all(home).expect("home should remove");
}

#[test]
fn doctor_report_includes_global_registry_error_for_malformed_config() {
    let repo = std::env::temp_dir().join(format!(
        "stateful-doctor-malformed-repo-{}",
        std::process::id()
    ));
    let home = std::env::temp_dir().join(format!(
        "stateful-doctor-malformed-home-{}",
        std::process::id()
    ));
    if repo.exists() {
        fs::remove_dir_all(&repo).expect("old repo should be removable");
    }
    if home.exists() {
        fs::remove_dir_all(&home).expect("old home should be removable");
    }
    fs::create_dir_all(&repo).expect("repo should write");
    let paths = stateful_cli::GlobalPaths::new(&home);
    fs::create_dir_all(
        paths
            .config_yml
            .parent()
            .expect("global config should have a parent"),
    )
    .expect("global config parent should write");
    fs::write(&paths.config_yml, "repos: [\n").expect("malformed global config should write");

    let report = doctor_report_with_global(&repo, &paths);

    assert!(!report.repo_enabled);
    let error = report
        .global_registry_error
        .as_deref()
        .expect("global registry error should be reported");
    assert!(error.contains("failed to parse repo registry config"));
    fs::remove_dir_all(repo).expect("repo should remove");
    fs::remove_dir_all(home).expect("home should remove");
}
