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
    let command_policy_skill_path = fixture
        .codex_config_parent()
        .join("skills/stateful-command-policy/SKILL.md");
    let dispatching_skill_path = fixture
        .codex_config_parent()
        .join("skills/dispatching-parallel-agents/SKILL.md");

    assert!(plan.summary.contains("dry-run"));
    assert!(applied.summary.contains("dry-run"));
    assert!(plan.files.contains(&fixture.paths.home));
    assert!(plan.files.contains(&fixture.paths.state_db));
    assert!(plan.files.contains(&fixture.codex_config));
    assert!(plan.files.contains(&command_policy_skill_path));
    assert!(plan.files.contains(&dispatching_skill_path));
    assert!(!fixture.paths.home.exists());
    assert!(!fixture.codex_config.exists());
    assert!(!dispatching_skill_path.exists());
}

#[test]
fn install_omp_dry_run_plans_command_policy_skill_without_writing() {
    let fixture = TestFixture::new("omp-dry-run-skill");
    let options = fixture.omp_options(false);

    let plan = plan_omp_install(&options).expect("omp install should plan");
    let applied = apply_omp_install(options).expect("dry-run omp install should succeed");
    let command_policy_skill_path = fixture
        .omp_agent_dir()
        .join("skills")
        .join("stateful-command-policy")
        .join("SKILL.md");
    let dispatching_skill_path = fixture
        .omp_agent_dir()
        .join("skills")
        .join("dispatching-parallel-agents")
        .join("SKILL.md");

    assert!(plan.summary.contains("dry-run"));
    assert!(applied.summary.contains("dry-run"));
    assert!(plan.files.contains(&command_policy_skill_path));
    assert!(!plan.files.contains(&dispatching_skill_path));
    assert!(!fixture.paths.home.exists());
    assert!(!command_policy_skill_path.exists());
    assert!(!dispatching_skill_path.exists());
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
        "env_vars = [\"CODEX_THREAD_ID\", \"STATEFUL_CODEX_RUN_ID\", \"STATEFUL_SERVER_URL\", \"STATEFUL_SERVER_TOKEN\"]"
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
            "[mcp_servers.stateful.tools.state_claim_acquire]\napproval_mode = \"approve\""
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
    let omp_dispatching_skill = omp_agent_dir
        .join("skills")
        .join("dispatching-parallel-agents")
        .join("SKILL.md");

    assert!(omp_config.is_file());
    assert!(omp_mcp.is_file());
    assert!(omp_extension.is_file());
    assert!(omp_skill.is_file());
    assert!(!omp_dispatching_skill.exists());
    assert!(
        fs::read_to_string(&omp_config)
            .expect("omp config should read")
            .contains("stateful-omp-extension.js")
    );
    let config = fs::read_to_string(&omp_config).expect("omp config should read");
    assert!(config.contains("stateful-omp-extension.js"));
    assert!(config.contains(
        "tools:\n  approvalMode: yolo\nstateful:\n  autoApprove: false\neval:\n  py: false\n  js: false\n  rb: false\n  jl: false\nbash:\n  enabled: true\n",
    ));
    assert!(!config.contains("approval:"));
    assert!(
        fs::read_to_string(&omp_mcp)
            .expect("omp mcp should read")
            .contains("\"mcpServers\"")
    );
    let extension = fs::read_to_string(&omp_extension).expect("omp extension should read");
    assert!(extension.contains("STATEFUL_BENCHMARK_SOURCE_BLOCK_PATTERNS"));
    assert!(extension.contains("function benchmarkSourceBlockReason(event)"));
    assert!(extension.contains("function benchmarkSourcePatternMatches(text, pattern)"));
    assert!(extension.contains("upstream(?:\\/|[^a-z0-9_-]|$)"));
    assert!(!extension.contains("upstream(?:\\\\/|"));
    let benchmark_block = extension
        .find("const benchmarkBlockReason = benchmarkSourceBlockReason(event);")
        .expect("benchmark guard should run in pre-tool hook");
    let stateful_pre_tool = extension
        .find("const decision = runStatefulHook(\"pre-tool-use\"")
        .expect("stateful pre-tool hook should still run");
    assert!(
        benchmark_block < stateful_pre_tool,
        "benchmark guard should block before Stateful reservation handling"
    );
    assert!(extension.contains(
        "if (benchmarkBlockReason) return { block: true, reason: benchmarkBlockReason };"
    ));
    assert!(!extension.contains("@sinclair/typebox"));
    assert!(!extension.contains("Type.Object"));
    assert!(extension.contains("export default function statefulOmpExtension"));
    assert!(extension.contains("[\"hook\", \"omp\", event]"));
    assert!(extension.contains("function detectSessionId(event, ctx)"));
    assert!(extension.contains("event?.sessionId"));
    assert!(!extension.contains("ctx?.sessionManager?.session?.id"));
    assert!(extension.contains("event?.session_id"));
    assert!(extension.contains("sessionManager?.getSessionFile?.()"));
    assert!(extension.contains("sessionManager?.getLeafId?.()"));
    assert!(extension.contains("function sessionIdFromString(value, prefix = \"omp\")"));
    assert!(extension.contains("function sessionIdFromSessionFile(sessionFile)"));
    assert!(!extension.contains("|| \"omp-session\""));
    assert!(!extension.contains("process.env.STATEFUL_SESSION_ID ||"));
    assert!(extension.contains("pi.registerTool"));
    assert!(extension.contains("name: \"lazy_edit_resume\""));
    assert!(extension.contains("name: \"lazy_write_resume\""));
    assert!(extension.contains("name: \"lazy_bash_resume\""));
    assert!(extension.contains("lazyBashOperations"));
    assert!(extension.contains("rememberLazyBashOperation(event, ctx, decision)"));
    assert!(extension.contains("Queued lazy bash operation_id"));
    assert!(
        extension.contains(
            "Resume a blocked OMP Bash command after approving an external sandbox grant."
        )
    );
    assert!(!extension.contains("name: \"sandbox_bash\""));
    assert!(!extension.contains("name: \"ext_ro_bash\""));
    assert!(!extension.contains("name: \"ext_rw_bash\""));
    assert!(!extension.contains("name: \"process_find\""));
    assert!(!extension.contains("name: \"sandbox_job_poll\""));
    assert!(extension.contains("applyOmpLinePatch"));
    assert!(extension.contains("lazyEditOperations"));
    assert!(extension.contains("let lazyEditOperationCounter = 0"));
    assert!(extension.contains("nextLazyEditOperationId()"));
    assert!(extension.contains("lazyWriteOperations"));
    assert!(extension.contains("nextLazyWriteOperationId()"));
    assert!(extension.contains("reservation or claim is ready"));
    assert!(extension.contains("structuredLazyEditOperationId(decision)"));
    assert!(extension.contains("structuredLazyWriteOperationId(decision)"));
    assert!(extension.contains("function extractReservationId(reason)"));
    assert!(extension.contains("extractReservationId(decision?.reason)"));
    assert!(extension.contains("extractReservationId(decision?.message)"));
    assert!(extension.contains("Queued lazy write operation_id"));
    assert!(extension.contains("!target.includes(\":\")"));
    assert!(extension.contains("validateOmpLinePatchBases"));
    assert!(extension.contains("line === \"*** Begin Patch\""));
    assert!(extension.contains("import { spawnSync } from \"node:child_process\""));
    assert!(extension.contains("import { createHash } from \"node:crypto\""));
    assert!(extension.contains(
        "import { closeSync, existsSync, mkdirSync, openSync, readFileSync, readSync, statSync, writeFileSync } from \"node:fs\""
    ));
    assert!(
        extension.contains(
            "import { basename, delimiter, dirname, extname, resolve } from \"node:path\""
        )
    );
    assert!(extension.contains("import { fileURLToPath } from \"node:url\""));
    assert!(
        extension
            .contains("const OMP_AGENT_CONFIG = resolve(EXTENSION_DIR, \"..\", \"config.yml\")")
    );
    assert!(extension.contains("process.env.HOME"));
    assert!(extension.contains(".omp/profiles/stateful/agent/config.yml"));
    assert!(extension.contains("function startReservationStream(pi, stream)"));
    assert!(extension.contains("/v1/notifications/stream?session_id="));
    assert!(extension.contains("customType: \"stateful_reservation_ready\""));
    assert!(extension.contains("purpose: \" + purpose.trim()"));
    assert!(extension.contains("const purpose = payload.purpose"));
    assert!(extension.contains("typeof purpose === \"string\" && purpose.trim().length > 0"));
    assert!(extension.contains("startReservationStream(pi, result?.notifications_stream)"));
    assert!(extension.contains("stopReservationStream();"));
    assert!(extension.contains("ctx.ui.confirm"));
    assert!(extension.contains("const externalBashGrants = new Map()"));
    assert!(extension.contains("function externalGrantDescriptor(params)"));
    assert!(extension.contains("async function ensureExternalBashGrant(ctx, params, signal)"));
    assert!(extension.contains("pi.on(\"tool_call\""));
    assert!(extension.contains("function statefulBashPassthroughDecision"));
    assert!(extension.contains("let verifiedBareStatefulPath = null"));
    assert!(extension.contains("function statefulBinaryDigest(path)"));
    assert!(extension.contains("verifyBareStateful(ctx.cwd)"));
    assert!(extension.contains("isTrustedStatefulCommand(words[0])"));
    assert!(extension.contains("event?.toolName !== \"bash\""));
    assert!(extension.contains("ensureExternalBashGrant(ctx, params, signal)"));
    assert!(extension.contains("Approve external sandbox grant"));
    assert!(
        extension.contains("Raw command text is intentionally hidden from this approval prompt.")
    );
    assert!(!extension.contains("\"Command:\""));
    assert!(!extension.contains("\"Sandbox invocation:\""));
    assert!(
        extension.contains("Built-in Bash external sandbox command requires OMP UI confirmation")
    );
    assert!(extension.contains("delete process.env.STATEFUL_SESSION_ID"));
    assert!(extension.contains("pre-tool-use"));
    assert!(extension.contains("decision: \"block\""));
    assert!(extension.contains("if (decision.decision === \"prompt\" && !shouldAutoApproveStatefulPrompt(ctx, event.input || {}))"));
    assert!(extension.contains("ctx?.ui?.confirm"));
    assert!(extension.contains("function shouldAutoApproveStatefulPrompt(ctx, _params)"));
    assert!(extension.contains("ctx?.config?.stateful?.autoApprove"));
    assert!(extension.contains("ctx?.config?.[\"stateful.autoApprove\"]"));
    assert!(extension.contains("function configTextAutoApprove(text)"));
    assert!(extension.contains("statefulConfigFileAutoApprove()"));
    assert!(extension.contains("value === \"true\""));
    assert!(!extension.contains("params?.auto_approve === true"));
    assert!(extension.contains("function recordExternalBashGrant(params, now)"));
    assert!(extension.contains("function approveExternalBashGrantWithoutPrompt(params)"));
    let command_policy_skill = fs::read_to_string(&omp_skill).expect("omp skill should read");
    let source_command_policy_skill = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/stateful-command-policy/SKILL.md"),
    )
    .expect("source stateful command policy skill should exist");
    assert_eq!(command_policy_skill, source_command_policy_skill);
    assert!(command_policy_skill.contains("name: stateful-command-policy"));
    assert!(plan.files.contains(&omp_skill));
    for (name, marker) in [
        ("omp-tools.md", "Use OMP-Native Stateful Tools"),
        (
            "sandbox-tools.md",
            "Choose the narrowest existing entry point",
        ),
        ("denial-recovery.md", "Denials are the API"),
        ("subagent-write-recovery.md", "Subagent Write Recovery"),
    ] {
        let support_path = omp_agent_dir
            .join("skills/stateful-command-policy")
            .join(name);
        let support_file =
            fs::read_to_string(&support_path).expect("OMP support file should exist");
        let source_support_file = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/stateful-command-policy")
                .join(name),
        )
        .expect("source support file should exist");
        assert_eq!(support_file, source_support_file);
        assert!(support_file.contains(marker));
        assert!(plan.files.contains(&support_path));
    }
    assert!(!omp_dispatching_skill.exists());
    assert!(!plan.files.contains(&omp_dispatching_skill));
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
        update: false,
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
    assert_eq!(count(&config, "approvalMode: yolo"), 1);
    assert_eq!(count(&config, "autoApprove: false"), 1);
    assert_eq!(count(&config, "approval:"), 0);
    assert_eq!(count(&config, "external_bash:"), 0);
    assert_eq!(count(&config, "\n  py: false"), 1);
    assert_eq!(count(&config, "\n  js: false"), 1);
    assert_eq!(count(&config, "\n  rb: false"), 1);
    assert_eq!(count(&config, "\n  jl: false"), 1);
    assert_eq!(count(&config, "\n  enabled: true"), 1);
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
fn install_omp_yes_preserves_existing_config_and_uses_yolo_approval() {
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
    assert!(config.contains("tools:\n  approvalMode: yolo\n"));
    assert!(config.contains("stateful:\n  autoApprove: false\n"));
    assert!(!config.contains("approval:"));
    assert!(!config.contains("task: allow"));
    assert!(!config.contains("sandbox_bash: allow"));
    assert!(!config.contains("ext_ro_bash: allow"));
    assert!(!config.contains("ext_rw_bash: allow"));
    assert!(!config.contains("external_bash:"));
    assert!(config.contains("eval:\n  py: false\n  js: false\n  rb: false\n  jl: false\n"));
    assert!(config.contains("bash:\n  enabled: true\n"));
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
fn install_omp_yes_removes_existing_tool_approval_without_update() {
    let fixture = TestFixture::new("omp-install-tools-preserve");
    let omp_agent_dir = fixture.omp_agent_dir();
    fs::create_dir_all(&omp_agent_dir).expect("omp dir should create");
    let omp_config = omp_agent_dir.join("config.yml");
    fs::write(
        &omp_config,
        "model: gpt-5.5\ntools:\n  approvalMode: yolo\n  approval:\n    task: prompt\n    bash: prompt\n    edit: prompt\nstateful:\n  autoApprove: true\nbash:\n  enabled: false\n",
    )
    .expect("existing config should write");

    apply_omp_install(fixture.omp_options(true)).expect("omp install should apply");

    let config = fs::read_to_string(&omp_config).expect("omp config should read");
    assert!(config.contains("tools:\n  approvalMode: yolo\n"));
    assert!(config.contains("stateful:\n  autoApprove: true\n"));
    assert!(!config.contains("approval:"));
    assert!(!config.contains("task: prompt"));
    assert!(!config.contains("bash: prompt"));
    assert!(!config.contains("edit: prompt"));
    assert!(!config.contains("task: allow"));
    assert!(!config.contains("sandbox_bash: allow"));
    assert!(!config.contains("ext_ro_bash: allow"));
    assert!(!config.contains("ext_rw_bash: allow"));
    assert!(!config.contains("external_bash:"));
    assert!(config.contains("eval:\n  py: false\n  js: false\n  rb: false\n  jl: false\n"));
    assert!(config.contains("bash:\n  enabled: true\n"));
    assert_eq!(count(&config, "approvalMode:"), 1);
    assert_eq!(count(&config, "approval:"), 0);
}

#[test]
fn install_omp_update_removes_existing_tool_approval() {
    let fixture = TestFixture::new("omp-install-tools-update");
    let omp_agent_dir = fixture.omp_agent_dir();
    fs::create_dir_all(&omp_agent_dir).expect("omp dir should create");
    let omp_config = omp_agent_dir.join("config.yml");
    fs::write(
        &omp_config,
        "model: gpt-5.5\ntools:\n  approvalMode: yolo\n  approval:\n    task: prompt\n    sandbox_bash: prompt\n    external_bash: prompt\n    ext_ro_bash: prompt\n    ext_rw_bash: prompt\n    edit: prompt\neval:\n  py: true\n  js: true\n  rb: true\n  jl: true\nbash:\n  enabled: true\nstateful:\n  autoApprove: true\n",
    )
    .expect("existing config should write");

    let mut options = fixture.omp_options(true);
    options.update = true;
    apply_omp_install(options).expect("omp install with --update should apply");

    let config = fs::read_to_string(&omp_config).expect("omp config should read");
    assert!(config.contains("tools:\n  approvalMode: yolo\n"));
    assert!(config.contains("stateful:\n  autoApprove: false\n"));
    assert!(!config.contains("approval:"));
    assert!(!config.contains("task: allow"));
    assert!(!config.contains("sandbox_bash: allow"));
    assert!(!config.contains("ext_ro_bash: allow"));
    assert!(!config.contains("ext_rw_bash: allow"));
    assert!(!config.contains("external_bash:"));
    assert!(!config.contains("edit: prompt"));
    assert!(config.contains("eval:\n  py: false\n  js: false\n  rb: false\n  jl: false\n"));
    assert!(config.contains("bash:\n  enabled: true\n"));
    assert_eq!(count(&config, "approvalMode: yolo"), 1);
    assert_eq!(count(&config, "approval:"), 0);
}

#[test]
fn install_omp_rejects_invalid_existing_yaml_without_writing() {
    let fixture = TestFixture::new("omp-install-invalid-yaml");
    let omp_agent_dir = fixture.omp_agent_dir();
    fs::create_dir_all(&omp_agent_dir).expect("omp dir should create");
    let omp_config = omp_agent_dir.join("config.yml");
    let existing = "model: [unterminated\n";
    fs::write(&omp_config, existing).expect("existing config should write");

    let error = apply_omp_install(fixture.omp_options(true)).expect_err("invalid YAML should fail");

    assert!(error.to_string().contains("invalid OMP config YAML"));
    assert_eq!(
        fs::read_to_string(&omp_config).expect("config should remain readable"),
        existing
    );
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
    let dispatching_skill_path = fixture
        .codex_config_parent()
        .join("skills/dispatching-parallel-agents/SKILL.md");
    let command_policy_skill =
        fs::read_to_string(&skill_path).expect("global command policy skill should exist");
    let source_command_policy_skill = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/stateful-command-policy/SKILL.md"),
    )
    .expect("source stateful command policy skill should exist");
    assert_eq!(command_policy_skill, source_command_policy_skill);
    assert!(command_policy_skill.contains("name: stateful-command-policy"));
    assert!(command_policy_skill.contains("Support Files"));
    assert!(command_policy_skill.contains("state_reservation_declare"));
    assert!(command_policy_skill.contains("state_claim_acquire"));
    assert!(command_policy_skill.contains("Runtime-specific wrappers are aliases"));
    for (name, marker) in [
        ("omp-tools.md", "Use OMP-Native Stateful Tools"),
        (
            "sandbox-tools.md",
            "Choose the narrowest existing entry point",
        ),
        ("denial-recovery.md", "Denials are the API"),
        ("subagent-write-recovery.md", "Subagent Write Recovery"),
    ] {
        let support_path = fixture
            .codex_config_parent()
            .join("skills/stateful-command-policy")
            .join(name);
        let support_file = fs::read_to_string(&support_path).expect("support file should exist");
        let source_support_file = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/stateful-command-policy")
                .join(name),
        )
        .expect("source support file should exist");
        assert_eq!(support_file, source_support_file);
        assert!(support_file.contains(marker));
    }
    assert!(!command_policy_skill.contains("Reservation declarations replace"));
    assert!(
        !command_policy_skill.contains("--fs write-targets --network enabled --write-dir target")
    );
    let dispatching_skill =
        fs::read_to_string(&dispatching_skill_path).expect("global dispatching skill should exist");
    let source_dispatching_skill = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/dispatching-parallel-agents/SKILL.md"),
    )
    .expect("source dispatching skill should exist");
    assert_eq!(dispatching_skill, source_dispatching_skill);
    assert!(dispatching_skill.contains("name: dispatching-parallel-agents"));
    assert!(dispatching_skill.contains("Dispatch one agent per independent problem domain"));

    let plan =
        apply_codex_install(fixture.codex_options(true)).expect("install should be idempotent");
    assert!(plan.files.contains(&skill_path));
    for name in [
        "omp-tools.md",
        "sandbox-tools.md",
        "denial-recovery.md",
        "subagent-write-recovery.md",
    ] {
        assert!(
            plan.files.contains(
                &fixture
                    .codex_config_parent()
                    .join("skills/stateful-command-policy")
                    .join(name)
            )
        );
    }
    assert!(plan.files.contains(&dispatching_skill_path));
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
            update: false,
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
