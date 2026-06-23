use std::{
    fs,
    path::{Path, PathBuf},
};

use stateful_cli::{
    CodexInstallOptions, GlobalPaths, InstallOptions, OmpInstallOptions, RepoRegistry,
    apply_codex_install, apply_global_install, apply_omp_install, plan_codex_install,
    plan_global_install, plan_omp_install,
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
fn install_dry_run_does_not_write_files() {
    let fixture = TestFixture::new("dry-run");
    let options = fixture.options(false);

    let plan = plan_global_install(&options).expect("install should plan");
    let applied = apply_global_install(options).expect("dry-run install should succeed");

    assert!(plan.summary.contains("dry-run"));
    assert!(applied.summary.contains("dry-run"));
    assert!(plan.files.contains(&fixture.paths.home));
    assert!(plan.files.contains(&fixture.paths.state_db));
    assert!(!plan.files.contains(&fixture.codex_config));
    assert!(!fixture.paths.home.exists());
    assert!(!fixture.codex_config.exists());
}

#[test]
fn install_codex_dry_run_plans_codex_config_without_writing() {
    let fixture = TestFixture::new("codex-dry-run");
    let options = fixture.codex_options(false);

    let plan = plan_codex_install(&options).expect("codex install should plan");
    let applied = apply_codex_install(options).expect("dry-run codex install should succeed");
    let skill_path = fixture
        .codex_config_parent()
        .join("skills/stateful-command-policy/SKILL.md");

    assert!(plan.summary.contains("dry-run"));
    assert!(applied.summary.contains("dry-run"));
    assert!(plan.files.contains(&fixture.paths.home));
    assert!(plan.files.contains(&fixture.paths.state_db));
    assert!(plan.files.contains(&fixture.codex_config));
    assert!(plan.files.contains(&skill_path));
    assert!(!fixture.paths.home.exists());
    assert!(!fixture.codex_config.exists());
}

#[test]
fn install_omp_dry_run_plans_command_policy_skill_without_writing() {
    let fixture = TestFixture::new("omp-dry-run-skill");
    let options = fixture.omp_options(false);

    let plan = plan_omp_install(&options).expect("omp install should plan");
    let applied = apply_omp_install(options).expect("dry-run omp install should succeed");
    let skill_path = fixture
        .omp_agent_dir()
        .join("skills")
        .join("stateful-command-policy")
        .join("SKILL.md");

    assert!(plan.summary.contains("dry-run"));
    assert!(applied.summary.contains("dry-run"));
    assert!(plan.files.contains(&skill_path));
    assert!(!fixture.paths.home.exists());
    assert!(!skill_path.exists());
}

#[test]
fn install_yes_creates_global_files_and_database() {
    let fixture = TestFixture::new("yes");

    apply_global_install(fixture.options(true)).expect("install should apply");

    assert!(fixture.paths.home.is_dir());
    assert!(fixture.paths.runtime_dir.is_dir());
    assert!(fixture.paths.repos_dir.is_dir());
    assert!(fixture.paths.config_yml.is_file());
    assert!(fixture.paths.state_db.is_file());
    assert!(!fixture.codex_config.exists());
    assert!(!fixture.codex_config_parent().join("hooks.json").exists());

    let registry = RepoRegistry::load(&fixture.paths).expect("registry should load");
    assert_eq!(registry, RepoRegistry::default());

    let store = stateful_store::Store::open(&fixture.paths.state_db).expect("store should open");
    assert_eq!(store.event_count().expect("event count should load"), 0);
}

#[test]
fn install_yes_does_not_open_existing_state_database() {
    let fixture = TestFixture::new("existing-db");
    fs::create_dir_all(&fixture.paths.home).expect("stateful home should be creatable");
    fs::write(&fixture.paths.state_db, b"owned by stateful server")
        .expect("existing state db should be writable");

    apply_global_install(fixture.options(true)).expect("install should apply");

    assert_eq!(
        fs::read(&fixture.paths.state_db).expect("existing state db should reread"),
        b"owned by stateful server"
    );
}

#[test]
fn install_codex_yes_creates_global_files_and_merges_codex_config() {
    let fixture = TestFixture::new("codex-yes");

    apply_codex_install(fixture.codex_options(true)).expect("install should apply");

    assert!(fixture.paths.home.is_dir());
    assert!(fixture.paths.runtime_dir.is_dir());
    assert!(fixture.paths.repos_dir.is_dir());
    assert!(fixture.paths.config_yml.is_file());
    assert!(fixture.paths.state_db.is_file());
    assert!(fixture.codex_config.is_file());
    assert!(!fixture.codex_config_parent().join("hooks.json").exists());
    assert!(fixture.codex_rules_path().is_file());

    let registry = RepoRegistry::load(&fixture.paths).expect("registry should load");
    assert_eq!(registry, RepoRegistry::default());

    let store = stateful_store::Store::open(&fixture.paths.state_db).expect("store should open");
    assert_eq!(store.event_count().expect("event count should load"), 0);

    let first_config = fs::read_to_string(&fixture.codex_config).expect("codex config should read");
    assert!(first_config.contains("# stateful-core-global-install"));
    assert!(first_config.contains("[mcp_servers.stateful]"));
    assert!(first_config.contains("command = \"/opt/stateful/bin/stateful\""));
    assert!(first_config.contains(
        "env_vars = [\"CODEX_THREAD_ID\", \"STATEFUL_CODEX_RUN_ID\", \"STATEFUL_SESSION_ID\", \"STATEFUL_SERVER_URL\", \"STATEFUL_SERVER_TOKEN\"]"
    ));
    assert!(first_config.contains(
        "approval_policy = { granular = { sandbox_approval = false, rules = true, mcp_elicitations = false, request_permissions = false, skill_approval = false } }"
    ));
    assert!(first_config.contains("default_tools_approval_mode = \"approve\""));
    assert!(
        !first_config.contains("[mcp_servers.stateful.tools.state_current_read]"),
        "stateful MCP tools should inherit the default approve mode"
    );
    assert!(
        !first_config.contains(
            "[mcp_servers.stateful.tools.state_lease_acquire]\napproval_mode = \"approve\""
        )
    );
    assert!(first_config.contains("hook codex pre-tool-use"));
    assert!(!first_config.contains("hook pre-tool-use"));
    assert!(first_config.contains("[[hooks.PreToolUse]]\nmatcher = \".*\""));
    assert!(first_config.contains(
        "[[hooks.PostToolUse]]\nmatcher = \"Bash|apply_patch|Edit|Write|file_change|mcp__filesystem__.*\""
    ));
    assert_eq!(count(&first_config, "[features]"), 1);

    apply_codex_install(fixture.codex_options(true)).expect("install should be idempotent");

    let second_config =
        fs::read_to_string(&fixture.codex_config).expect("codex config should reread");
    assert_eq!(count(&second_config, "# stateful-core-global-install"), 1);
    assert_eq!(count(&second_config, "[mcp_servers.stateful]"), 1);
    assert_eq!(count(&second_config, "default_tools_approval_mode"), 1);
    assert_eq!(count(&second_config, "approval_policy = { granular"), 1);
    assert!(!second_config.contains("[mcp_servers.stateful.tools."));
    assert_eq!(count(&second_config, "[features]"), 1);
    assert_eq!(count(&second_config, "[[hooks.PreToolUse]]"), 1);
}

#[test]
fn install_omp_yes_creates_extension_and_mcp_config() {
    let fixture = TestFixture::new("omp-install");

    let plan = apply_omp_install(fixture.omp_options(true)).expect("omp install should apply");

    let omp_agent_dir = fixture.omp_agent_dir();
    let omp_config = omp_agent_dir.join("config.yml");
    let omp_mcp = omp_agent_dir.join("mcp.json");
    let omp_extension = omp_agent_dir
        .join("extensions")
        .join("stateful-omp-extension.js");
    let omp_skill = omp_agent_dir
        .join("skills")
        .join("stateful-command-policy")
        .join("SKILL.md");

    assert!(omp_config.is_file());
    assert!(omp_mcp.is_file());
    assert!(omp_extension.is_file());
    assert!(omp_skill.is_file());
    assert!(
        fs::read_to_string(&omp_config)
            .expect("omp config should read")
            .contains("stateful-omp-extension.js")
    );
    let config = fs::read_to_string(&omp_config).expect("omp config should read");
    assert!(config.contains("stateful-omp-extension.js"));
    assert!(config.contains(
        "tools:\n  approvalMode: write\n  approval:\n    bash: prompt\n    python: prompt\n    task: allow\n    external_bash: prompt\n",
    ));
    assert!(
        fs::read_to_string(&omp_mcp)
            .expect("omp mcp should read")
            .contains("\"mcpServers\"")
    );
    let extension = fs::read_to_string(&omp_extension).expect("omp extension should read");
    assert!(extension.contains("export default function statefulOmpExtension"));
    assert!(extension.contains("[\"hook\", \"omp\", event]"));
    assert!(extension.contains("event?.sessionId || ctx?.sessionManager?.session?.id"));
    assert!(extension.contains("pi.registerTool"));
    assert!(extension.contains("name: \"external_bash\""));
    assert!(extension.contains("ctx.ui.confirm"));
    assert!(extension.contains("[\"sandbox\", \"run\", \"--fs\", \"external\""));
    assert!(extension.contains("process.env.STATEFUL_SESSION_ID = id"));
    assert!(extension.contains("pre-tool-use"));
    assert!(extension.contains("decision: \"block\""));
    assert!(extension.contains("decision.decision === \"prompt\""));
    assert!(extension.contains("ctx?.ui?.confirm"));
    let command_policy_skill = fs::read_to_string(&omp_skill).expect("omp skill should read");
    let source_command_policy_skill = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/stateful-command-policy/SKILL.md"),
    )
    .expect("source stateful command policy skill should exist");
    assert_eq!(command_policy_skill, source_command_policy_skill);
    assert!(command_policy_skill.contains("name: stateful-command-policy"));
    assert!(plan.files.contains(&omp_skill));
    assert!(plan.files.iter().any(|path| path.ends_with("mcp.json")));
}

#[test]
fn install_omp_yes_can_target_user_omp_profile_separate_from_stateful_home() {
    let fixture = TestFixture::new("omp-install-default-home");
    let user_home = fixture.root.join("home");
    let stateful_home = user_home.join(".stateful_core");
    let paths = GlobalPaths::new(&stateful_home);
    let omp_agent_dir = user_home
        .join(".omp")
        .join("profiles")
        .join("stateful")
        .join("agent");

    let plan = apply_omp_install(OmpInstallOptions {
        yes: true,
        paths,
        binary_path: "/opt/stateful/bin/stateful".to_string(),
        project_config_path: None,
        omp_agent_dir: Some(omp_agent_dir.clone()),
    })
    .expect("omp install should apply");

    let misplaced_agent_dir = stateful_home
        .join(".omp")
        .join("profiles")
        .join("stateful")
        .join("agent");

    assert!(omp_agent_dir.join("config.yml").is_file());
    assert!(omp_agent_dir.join("mcp.json").is_file());
    assert!(
        omp_agent_dir
            .join("extensions")
            .join("stateful-omp-extension.js")
            .is_file()
    );
    assert!(plan.files.contains(&omp_agent_dir.join("config.yml")));
    assert!(!misplaced_agent_dir.exists());
}

#[test]
fn install_omp_yes_can_run_twice_without_existing_file_errors() {
    let fixture = TestFixture::new("omp-install-idempotent");
    let options = || fixture.omp_options(true);

    apply_omp_install(options()).expect("first omp install should apply");
    apply_omp_install(options()).expect("second omp install should be idempotent");

    let omp_agent_dir = fixture.omp_agent_dir();
    let omp_config = omp_agent_dir.join("config.yml");
    let omp_mcp = omp_agent_dir.join("mcp.json");
    let omp_extension = omp_agent_dir
        .join("extensions")
        .join("stateful-omp-extension.js");

    let config = fs::read_to_string(&omp_config).expect("omp config should read");
    assert_eq!(count(&config, "stateful-omp-extension.js"), 1);
    assert_eq!(count(&config, "approvalMode: write"), 1);
    assert_eq!(count(&config, "\n    bash: prompt"), 1);
    assert_eq!(count(&config, "python: prompt"), 1);
    assert_eq!(count(&config, "task: allow"), 1);
    assert_eq!(count(&config, "external_bash: prompt"), 1);
    assert!(
        fs::read_to_string(&omp_mcp)
            .expect("omp mcp should read")
            .contains("\"mcpServers\"")
    );
    assert!(
        fs::read_to_string(&omp_extension)
            .expect("omp extension should read")
            .contains("[\"hook\", \"omp\", event]")
    );
}

#[test]
fn install_omp_yes_preserves_existing_config_and_uses_write_approval() {
    let fixture = TestFixture::new("omp-install-existing");
    let omp_agent_dir = fixture.omp_agent_dir();
    fs::create_dir_all(&omp_agent_dir).expect("omp dir should create");
    let omp_config = omp_agent_dir.join("config.yml");
    fs::write(
        &omp_config,
        "model: gpt-5.5\nextensions:\n  - existing-extension.js\n",
    )
    .expect("existing config should write");

    apply_omp_install(fixture.omp_options(true)).expect("omp install should apply");

    let config = fs::read_to_string(&omp_config).expect("omp config should read");
    assert!(config.contains("model: gpt-5.5"));
    assert!(config.contains("existing-extension.js"));
    assert!(config.contains("stateful-omp-extension.js"));
    assert!(config.contains("tools:\n  approvalMode: write\n  approval:\n"));
    assert!(config.contains("bash: prompt"));
    assert!(config.contains("python: prompt"));
    assert!(config.contains("task: allow"));
    assert!(config.contains("external_bash: prompt"));
    let extension = fs::read_to_string(
        omp_agent_dir
            .join("extensions")
            .join("stateful-omp-extension.js"),
    )
    .expect("omp extension should read");
    assert!(extension.contains("function isYolo"));
    assert!(extension.contains("yolo: isYolo(event, ctx)"));
}

#[test]
fn install_omp_yes_merges_approval_mode_into_existing_tools_config() {
    let fixture = TestFixture::new("omp-install-tools");
    let omp_agent_dir = fixture.omp_agent_dir();
    fs::create_dir_all(&omp_agent_dir).expect("omp dir should create");
    let omp_config = omp_agent_dir.join("config.yml");
    fs::write(
        &omp_config,
        "model: gpt-5.5\ntools:\n  approvalMode: yolo\n  approval:\n    bash: prompt\n    edit: prompt\n",
    )
    .expect("existing config should write");

    apply_omp_install(fixture.omp_options(true)).expect("omp install should apply");

    let config = fs::read_to_string(&omp_config).expect("omp config should read");
    assert!(config.contains("tools:\n  approvalMode: write\n  approval:\n"));
    assert!(config.contains("bash: prompt"));
    assert!(config.contains("python: prompt"));
    assert!(config.contains("task: allow"));
    assert!(config.contains("edit: prompt"));
    assert_eq!(count(&config, "approvalMode: write"), 1);
    assert_eq!(count(&config, "\n    bash: prompt"), 1);
    assert_eq!(count(&config, "python: prompt"), 1);
    assert_eq!(count(&config, "task: allow"), 1);
    assert_eq!(count(&config, "external_bash: prompt"), 1);
}

#[test]
fn install_codex_yes_creates_sandbox_external_prompt_rule() {
    let fixture = TestFixture::new("codex-rules");

    apply_codex_install(fixture.codex_options(true)).expect("install should apply");

    let rules = fs::read_to_string(fixture.codex_rules_path()).expect("rules should read");
    assert!(rules.contains("prefix_rule("));
    assert!(rules.contains(
        "pattern = [\"/opt/stateful/bin/stateful\", \"sandbox\", \"run\", \"--fs\", \"external\"]"
    ));
    assert!(rules.contains("decision = \"prompt\""));
    assert!(rules.contains("stateful sandbox run --fs external"));
    assert!(rules.contains(
        "/opt/stateful/bin/stateful sandbox run --fs external --purpose 'install rebuilt binaries'"
    ));
    assert!(!rules.contains("external-run"));

    apply_codex_install(fixture.codex_options(true)).expect("install should be idempotent");

    let second_rules = fs::read_to_string(fixture.codex_rules_path()).expect("rules should reread");
    assert_eq!(count(&second_rules, "prefix_rule("), 1);
}

#[test]
fn install_codex_yes_creates_global_command_policy_skill() {
    let fixture = TestFixture::new("skill");

    apply_codex_install(fixture.codex_options(true)).expect("install should apply");

    let skill_path = fixture
        .codex_config_parent()
        .join("skills/stateful-command-policy/SKILL.md");
    let command_policy_skill =
        fs::read_to_string(&skill_path).expect("global command policy skill should exist");
    let source_command_policy_skill = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/stateful-command-policy/SKILL.md"),
    )
    .expect("source stateful command policy skill should exist");
    assert_eq!(command_policy_skill, source_command_policy_skill);
    assert!(command_policy_skill.contains("name: stateful-command-policy"));
    assert!(command_policy_skill.contains("Use canonical Stateful MCP tool names"));
    assert!(command_policy_skill.contains("state_intent_declare"));
    assert!(command_policy_skill.contains("state_lease_acquire"));
    assert!(command_policy_skill.contains("runtime-specific tool names"));
    assert!(
        command_policy_skill
            .contains("Do not run `stateful intent declare` or `stateful mcp call` through Bash")
    );
    assert!(command_policy_skill.contains("Intent declarations add"));
    assert!(command_policy_skill.contains("--fs build --network enabled"));
    assert!(command_policy_skill.contains("--write-dir <scratch-purpose>"));
    assert!(command_policy_skill.contains("state_intent_request"));
    assert!(command_policy_skill.contains("state_notifications_poll"));
    assert!(command_policy_skill.contains("state_resume_next"));
    assert!(command_policy_skill.contains("state_intent_claim"));
    assert!(!command_policy_skill.contains("Intent declarations replace"));
    assert!(
        !command_policy_skill.contains("--fs write-targets --network enabled --write-dir target")
    );

    let plan =
        apply_codex_install(fixture.codex_options(true)).expect("install should be idempotent");
    assert!(plan.files.contains(&skill_path));
    assert_eq!(
        fs::read_to_string(&skill_path).expect("global command policy skill should reread"),
        command_policy_skill
    );
}

#[test]
fn install_yes_backs_up_existing_codex_config_before_merge() {
    let fixture = TestFixture::new("backup");
    let existing = "[tools]\ncustom = true\n";
    fs::create_dir_all(fixture.codex_config_parent()).expect("codex dir should create");
    fs::write(&fixture.codex_config, existing).expect("existing config should write");

    apply_codex_install(fixture.codex_options(true)).expect("install should apply");

    let merged = fs::read_to_string(&fixture.codex_config).expect("merged config should read");
    assert!(merged.contains(existing));
    assert!(merged.contains("# stateful-core-global-install"));

    let backup = single_backup_for(&fixture.codex_config);
    let backup_contents = fs::read_to_string(backup).expect("backup should read");
    assert_eq!(backup_contents, existing);
}

#[test]
fn install_yes_preserves_existing_features_and_enables_hooks() {
    let fixture = TestFixture::new("features");
    let existing = "[features] # codex feature flags\nexperimental = true\nhooks = false\n\n[tools]\ncustom = true\n";
    fs::create_dir_all(fixture.codex_config_parent()).expect("codex dir should create");
    fs::write(&fixture.codex_config, existing).expect("existing config should write");

    apply_codex_install(fixture.codex_options(true)).expect("install should apply");

    let merged = fs::read_to_string(&fixture.codex_config).expect("merged config should read");
    assert_eq!(count(&merged, "[features]"), 1);
    assert_eq!(count(&merged, "hooks = true"), 1);
    assert!(!merged.contains("hooks = false"));
    assert!(merged.contains("[features] # codex feature flags"));
    assert!(merged.contains("experimental = true"));
    assert!(merged.contains("[tools]\ncustom = true"));
}

#[test]
fn install_yes_replaces_existing_approval_policy_for_sandbox_external_rules() {
    let fixture = TestFixture::new("approval-policy");
    let existing = "approval_policy = \"on-request\"\nmodel = \"gpt-5.5\"\n";
    fs::create_dir_all(fixture.codex_config_parent()).expect("codex dir should create");
    fs::write(&fixture.codex_config, existing).expect("existing config should write");

    apply_codex_install(fixture.codex_options(true)).expect("install should apply");

    let merged = fs::read_to_string(&fixture.codex_config).expect("merged config should read");
    assert!(!merged.contains("approval_policy = \"on-request\""));
    assert!(merged.contains("approval_policy = { granular = {"));
    assert_eq!(count(&merged, "approval_policy = "), 1);
    assert!(merged.contains("model = \"gpt-5.5\""));
}

#[test]
fn install_yes_preserves_quoted_project_tables() {
    let fixture = TestFixture::new("quoted-project");
    let existing =
        "[projects.\"/workspace/project\"]\ntrust_level = \"trusted\"\n\n[tools]\ncustom = true\n";
    fs::create_dir_all(fixture.codex_config_parent()).expect("codex dir should create");
    fs::write(&fixture.codex_config, existing).expect("existing config should write");

    apply_codex_install(fixture.codex_options(true)).expect("install should apply");

    let merged = fs::read_to_string(&fixture.codex_config).expect("merged config should read");
    assert!(merged.contains("[projects.\"/workspace/project\"]"));
    assert!(merged.contains("trust_level = \"trusted\""));
    assert!(merged.contains("[tools]\ncustom = true"));
    assert!(merged.contains("[mcp_servers.stateful]"));
}

#[test]
fn install_yes_rejects_existing_unmarked_stateful_mcp_server() {
    let fixture = TestFixture::new("mcp-conflict");
    let existing = "[mcp_servers.stateful] # existing server\ncommand = \"other\"\n";
    fs::create_dir_all(fixture.codex_config_parent()).expect("codex dir should create");
    fs::write(&fixture.codex_config, existing).expect("existing config should write");

    let error = apply_codex_install(fixture.codex_options(true))
        .expect_err("unmarked stateful mcp config should conflict");

    assert!(error.to_string().contains("mcp_servers.stateful"));
    assert_eq!(
        fs::read_to_string(&fixture.codex_config).expect("config should remain readable"),
        existing
    );
    assert!(backup_paths_for(&fixture.codex_config).is_empty());
}

#[test]
fn install_yes_rejects_existing_unmarked_stateful_mcp_tool_config() {
    let fixture = TestFixture::new("mcp-tool-conflict");
    let existing = "[mcp_servers.stateful.tools.state_current_read]\napproval_mode = \"approve\"\n";
    fs::create_dir_all(fixture.codex_config_parent()).expect("codex dir should create");
    fs::write(&fixture.codex_config, existing).expect("existing config should write");

    let error = apply_codex_install(fixture.codex_options(true))
        .expect_err("unmarked stateful mcp tool config should conflict");

    assert!(error.to_string().contains("mcp_servers.stateful"));
    assert_eq!(
        fs::read_to_string(&fixture.codex_config).expect("config should remain readable"),
        existing
    );
    assert!(backup_paths_for(&fixture.codex_config).is_empty());
}

#[test]
fn install_yes_rejects_quoted_stateful_mcp_table_header() {
    let fixture = TestFixture::new("quoted-mcp-conflict");
    let existing = "[\"mcp_servers\".\"stateful\"]\ncommand = \"other\"\n";
    fs::create_dir_all(fixture.codex_config_parent()).expect("codex dir should create");
    fs::write(&fixture.codex_config, existing).expect("existing config should write");

    let error = apply_codex_install(fixture.codex_options(true))
        .expect_err("quoted stateful mcp config should conflict");

    assert!(error.to_string().contains("unsupported"));
    assert_eq!(
        fs::read_to_string(&fixture.codex_config).expect("config should remain readable"),
        existing
    );
    assert!(backup_paths_for(&fixture.codex_config).is_empty());
}

#[test]
fn install_yes_rejects_quoted_stateful_mcp_table_header_with_escape() {
    let fixture = TestFixture::new("quoted-mcp-escape-conflict");
    let existing = "[\"mcp_servers\".\"state\\u0066ul\"]\ncommand = \"other\"\n";
    fs::create_dir_all(fixture.codex_config_parent()).expect("codex dir should create");
    fs::write(&fixture.codex_config, existing).expect("existing config should write");

    let error = apply_codex_install(fixture.codex_options(true))
        .expect_err("quoted escaped stateful mcp config should conflict");

    assert!(error.to_string().contains("unsupported"));
    assert_eq!(
        fs::read_to_string(&fixture.codex_config).expect("config should remain readable"),
        existing
    );
    assert!(backup_paths_for(&fixture.codex_config).is_empty());
}

#[test]
fn install_yes_rejects_quoted_features_table_header_with_escape() {
    let fixture = TestFixture::new("quoted-features-escape-conflict");
    let existing = "[\"feat\\u0075res\"]\nexperimental = true\n";
    fs::create_dir_all(fixture.codex_config_parent()).expect("codex dir should create");
    fs::write(&fixture.codex_config, existing).expect("existing config should write");

    let error = apply_codex_install(fixture.codex_options(true))
        .expect_err("quoted escaped features config should conflict");

    assert!(error.to_string().contains("unsupported"));
    assert_eq!(
        fs::read_to_string(&fixture.codex_config).expect("config should remain readable"),
        existing
    );
    assert!(backup_paths_for(&fixture.codex_config).is_empty());
}

#[test]
fn install_yes_rejects_malformed_marker_block_without_writing() {
    let fixture = TestFixture::new("malformed-marker");
    let existing = "# stateful-core-global-install\n[mcp_servers.stateful]\ncommand = \"old\"\n";
    fs::create_dir_all(fixture.codex_config_parent()).expect("codex dir should create");
    fs::write(&fixture.codex_config, existing).expect("existing config should write");

    let error = apply_codex_install(fixture.codex_options(true))
        .expect_err("unterminated stateful block should fail");

    assert!(error.to_string().contains("missing end marker"));
    assert_eq!(
        fs::read_to_string(&fixture.codex_config).expect("config should remain readable"),
        existing
    );
    assert!(backup_paths_for(&fixture.codex_config).is_empty());
}

#[test]
fn install_yes_idempotent_rerun_does_not_create_extra_backup() {
    let fixture = TestFixture::new("backup-idempotent");
    let existing = "[tools]\ncustom = true\n";
    fs::create_dir_all(fixture.codex_config_parent()).expect("codex dir should create");
    fs::write(&fixture.codex_config, existing).expect("existing config should write");

    apply_codex_install(fixture.codex_options(true)).expect("first install should apply");
    let first_merged = fs::read_to_string(&fixture.codex_config).expect("config should read");
    assert_eq!(backup_paths_for(&fixture.codex_config).len(), 1);

    apply_codex_install(fixture.codex_options(true)).expect("second install should be idempotent");

    let second_merged = fs::read_to_string(&fixture.codex_config).expect("config should reread");
    assert_eq!(second_merged, first_merged);
    assert_eq!(backup_paths_for(&fixture.codex_config).len(), 1);
}

#[test]
fn install_yes_shell_quotes_dangerous_binary_path() {
    let fixture = TestFixture::new("dangerous-binary");
    let mut options = fixture.codex_options(true);
    options.binary_path = "/opt/stateful dir/$(touch x)`cmd`/foo'bar/stateful".to_string();

    apply_codex_install(options).expect("install should quote dangerous shell chars");

    let config = fs::read_to_string(&fixture.codex_config).expect("codex config should read");
    assert!(config.contains(r##"command = "/opt/stateful dir/$(touch x)`cmd`/foo'bar/stateful""##));
    assert!(config.contains(
        r##"command = "'/opt/stateful dir/$(touch x)`cmd`/foo'\\''bar/stateful' hook codex pre-tool-use""##
    ));
    assert_eq!(count(&config, "[mcp_servers.stateful]"), 1);
}

#[test]
fn install_yes_rejects_binary_path_with_control_character() {
    let fixture = TestFixture::new("control-binary");
    let mut options = fixture.codex_options(true);
    options.binary_path = "/opt/stateful\nbin/stateful".to_string();

    let error = apply_codex_install(options).expect_err("control chars should be rejected");

    assert!(error.to_string().contains("control character"));
    assert!(!fixture.paths.home.exists());
    assert!(!fixture.codex_config.exists());
}

#[cfg(unix)]
#[test]
fn install_yes_preserves_existing_codex_config_file_mode() {
    let fixture = TestFixture::new("file-mode");
    let existing = "[tools]\ncustom = true\n";
    fs::create_dir_all(fixture.codex_config_parent()).expect("codex dir should create");
    fs::write(&fixture.codex_config, existing).expect("existing config should write");
    let mut permissions = fs::metadata(&fixture.codex_config)
        .expect("config metadata should read")
        .permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(&fixture.codex_config, permissions).expect("config mode should set");

    apply_codex_install(fixture.codex_options(true)).expect("install should apply");

    let config_mode = fs::metadata(&fixture.codex_config)
        .expect("config metadata should reread")
        .permissions()
        .mode()
        & 0o777;
    let backup = single_backup_for(&fixture.codex_config);
    let backup_mode = fs::metadata(backup)
        .expect("backup metadata should read")
        .permissions()
        .mode()
        & 0o777;

    assert_eq!(config_mode, 0o600);
    assert_eq!(backup_mode, 0o600);
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

fn single_backup_for(config_path: &Path) -> PathBuf {
    let backups = backup_paths_for(config_path);

    assert_eq!(backups.len(), 1, "expected one backup, got {backups:?}");
    backups.into_iter().next().expect("backup should exist")
}

fn backup_paths_for(config_path: &Path) -> Vec<PathBuf> {
    let parent = config_path
        .parent()
        .expect("config path should have parent");
    if !parent.exists() {
        return Vec::new();
    }

    let file_name = config_path
        .file_name()
        .and_then(|name| name.to_str())
        .expect("config file name should be utf-8");
    let prefix = format!("{file_name}.stateful-backup-");
    fs::read_dir(parent)
        .expect("codex config dir should read")
        .map(|entry| entry.expect("dir entry should read").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .collect()
}

struct TestFixture {
    root: PathBuf,
    paths: GlobalPaths,
    codex_config: PathBuf,
}

impl TestFixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "stateful-install-global-{name}-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("old fixture root should be removable");
        }
        fs::create_dir_all(&root).expect("fixture root should be creatable");

        let paths = GlobalPaths::new(root.join("home"));
        let codex_config = root.join("codex").join("config.toml");
        Self {
            root,
            paths,
            codex_config,
        }
    }

    fn options(&self, yes: bool) -> InstallOptions {
        InstallOptions {
            yes,
            paths: self.paths.clone(),
        }
    }

    fn codex_options(&self, yes: bool) -> CodexInstallOptions {
        CodexInstallOptions {
            yes,
            paths: self.paths.clone(),
            codex_config_path: self.codex_config.clone(),
            binary_path: "/opt/stateful/bin/stateful".to_string(),
        }
    }

    fn omp_agent_dir(&self) -> PathBuf {
        self.paths
            .home
            .join(".omp")
            .join("profiles")
            .join("stateful")
            .join("agent")
    }

    fn omp_options(&self, yes: bool) -> OmpInstallOptions {
        OmpInstallOptions {
            yes,
            paths: self.paths.clone(),
            binary_path: "/opt/stateful/bin/stateful".to_string(),
            project_config_path: None,
            omp_agent_dir: Some(self.omp_agent_dir()),
        }
    }

    fn codex_config_parent(&self) -> &Path {
        self.codex_config
            .parent()
            .expect("codex config should have a parent directory")
    }

    fn codex_rules_path(&self) -> PathBuf {
        self.codex_config_parent()
            .join("rules")
            .join("stateful.rules")
    }
}

impl Drop for TestFixture {
    fn drop(&mut self) {
        if self.root.exists() {
            fs::remove_dir_all(&self.root).expect("fixture root should be removable");
        }
    }
}
