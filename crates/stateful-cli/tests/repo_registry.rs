use std::fs;

use stateful_cli::{
    CodexMode, GlobalPaths, RepoRegistry, detect_git_root, disable_repo, enable_repo,
};

#[test]
fn enable_repo_registers_git_root_and_writes_repo_configs() {
    let fixture = TestFixture::new("enable");
    let repo = fixture.create_repo("repo");
    let nested = repo.join("src").join("app");
    fs::create_dir_all(&nested).expect("nested repo directory should be creatable");

    let entry = enable_repo(&fixture.paths, &nested, false).expect("repo should enable");
    let registry = RepoRegistry::load(&fixture.paths).expect("registry should load");
    let canonical_repo = repo
        .canonicalize()
        .expect("repo root should canonicalize after creation");

    assert_eq!(entry.root, canonical_repo);
    assert_eq!(
        entry.validation_config_path,
        canonical_repo.join(".stateful/validation.yml")
    );
    assert_eq!(
        entry.policy_config_path,
        canonical_repo.join(".stateful/config.yml")
    );
    assert_eq!(entry.codex_mode, CodexMode::Global);
    assert!(entry.enabled);
    assert!(!entry.enabled_at.is_empty());
    assert_eq!(registry.repos, vec![entry.clone()]);
    assert!(registry.is_enabled(&repo));
    assert_eq!(
        detect_git_root(&nested).expect("git root should be detected"),
        canonical_repo
    );

    assert!(repo.join(".stateful/config.yml").is_file());
    assert!(repo.join(".stateful/validation.yml").is_file());
    assert!(
        fixture
            .paths
            .repos_dir
            .join(format!("{}.json", entry.repo_id))
            .is_file()
    );

    let saved = fs::read_to_string(&fixture.paths.config_yml).expect("registry yml should exist");
    assert!(saved.contains("codex_mode: global"));

    let replaced = enable_repo(&fixture.paths, &repo, false).expect("repo should re-enable");
    let registry = RepoRegistry::load(&fixture.paths).expect("registry should reload");

    assert_eq!(replaced.repo_id, entry.repo_id);
    assert_eq!(registry.repos.len(), 1);
}

#[test]
fn disable_repo_marks_existing_entry_disabled() {
    let fixture = TestFixture::new("disable");
    let repo = fixture.create_repo("repo");
    let nested = repo.join("src");
    fs::create_dir_all(&nested).expect("nested repo directory should be creatable");

    let enabled = enable_repo(&fixture.paths, &repo, false).expect("repo should enable");
    disable_repo(&fixture.paths, &nested).expect("repo should disable");

    let registry = RepoRegistry::load(&fixture.paths).expect("registry should load");
    let entry = registry
        .repos
        .iter()
        .find(|entry| entry.repo_id == enabled.repo_id)
        .expect("enabled repo should remain registered");

    assert!(!entry.enabled);
    assert!(!registry.is_enabled(&repo));
}

#[test]
fn disable_repo_updates_repo_metadata_enabled_flag() {
    let fixture = TestFixture::new("disable-metadata");
    let repo = fixture.create_repo("repo");

    let enabled = enable_repo(&fixture.paths, &repo, false).expect("repo should enable");
    disable_repo(&fixture.paths, &repo).expect("repo should disable");

    let metadata_path = fixture
        .paths
        .repos_dir
        .join(format!("{}.json", enabled.repo_id));
    let metadata = fs::read_to_string(metadata_path).expect("repo metadata should exist");
    let metadata: serde_json::Value =
        serde_json::from_str(&metadata).expect("metadata should be valid json");

    assert_eq!(metadata["enabled"], false);
}

#[test]
fn repo_local_codex_uses_absolute_binary_path() {
    let fixture = TestFixture::new("repo-local-codex");
    let repo = fixture.create_repo("repo");

    let entry = enable_repo(&fixture.paths, &repo, true).expect("repo should enable");

    assert_eq!(entry.codex_mode, CodexMode::RepoLocal);
    assert!(!repo.join(".codex/hooks.json").exists());

    let config = fs::read_to_string(repo.join(".codex/config.toml")).expect("config should exist");
    assert!(config.contains("# stateful-core-owned"));
    assert!(config.contains("[mcp_servers.stateful]"));
    assert!(config.contains("[[hooks.PreToolUse]]"));
    assert!(!config.contains("$(git rev-parse --show-toplevel)/stateful"));
    assert!(
        config.contains("command = \"/"),
        "repo-local codex config should use absolute command paths, got {config:?}"
    );
}

#[test]
fn repo_local_codex_refuses_to_overwrite_existing_non_stateful_config() {
    let fixture = TestFixture::new("repo-local-existing-config");
    let repo = fixture.create_repo("repo");
    let codex_dir = repo.join(".codex");
    fs::create_dir_all(&codex_dir).expect("codex dir should be creatable");
    let config_path = codex_dir.join("config.toml");
    fs::write(&config_path, "[mcp_servers.other]\ncommand = \"other\"\n")
        .expect("existing codex config should be writable");

    let error = enable_repo(&fixture.paths, &repo, true)
        .expect_err("repo-local codex should refuse existing non-stateful config");

    assert!(
        error
            .to_string()
            .contains("would overwrite existing Codex config")
    );
    let config = fs::read_to_string(config_path).expect("existing config should remain readable");
    assert_eq!(config, "[mcp_servers.other]\ncommand = \"other\"\n");
    assert!(!repo.join(".stateful/config.yml").exists());
    assert!(!repo.join(".stateful/validation.yml").exists());
}

#[test]
fn repo_local_codex_refuses_similar_non_stateful_hooks_without_side_effects() {
    let fixture = TestFixture::new("repo-local-similar-hooks");
    let repo = fixture.create_repo("repo");
    let codex_dir = repo.join(".codex");
    fs::create_dir_all(&codex_dir).expect("codex dir should be creatable");
    let hooks_path = codex_dir.join("hooks.json");
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

    let error = enable_repo(&fixture.paths, &repo, true)
        .expect_err("repo-local codex should refuse similar non-stateful hooks");

    assert!(
        error
            .to_string()
            .contains("would overwrite existing Codex config")
    );
    let saved_hooks =
        fs::read_to_string(hooks_path).expect("existing hooks should remain readable");
    assert_eq!(saved_hooks, hooks);
    assert!(!repo.join(".stateful/config.yml").exists());
    assert!(!repo.join(".stateful/validation.yml").exists());
}

#[test]
fn repo_local_codex_replaces_unmarked_legacy_stateful_codex_files() {
    let fixture = TestFixture::new("repo-local-legacy-codex");
    let repo = fixture.create_repo("repo");
    let codex_dir = repo.join(".codex");
    fs::create_dir_all(&codex_dir).expect("codex dir should be creatable");
    fs::write(
        codex_dir.join("hooks.json"),
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
        codex_dir.join("config.toml"),
        r#"[mcp_servers.stateful]
command = "stateful"
args = ["mcp", "serve"]
startup_timeout_sec = 20

[mcp_servers.stateful.tools.state_intent_declare]
approval_mode = "approve"
"#,
    )
    .expect("legacy config should be writable");

    let entry = enable_repo(&fixture.paths, &repo, true).expect("repo should enable");

    assert_eq!(entry.codex_mode, CodexMode::RepoLocal);
    assert!(!repo.join(".codex/hooks.json").exists());
    let config = fs::read_to_string(repo.join(".codex/config.toml")).expect("config should exist");
    assert!(config.contains("# stateful-core-owned"));
    assert!(config.contains("[[hooks.PreToolUse]]"));
    assert!(repo.join(".stateful/config.yml").exists());
    assert!(repo.join(".stateful/validation.yml").exists());
}

#[test]
fn enabled_lookup_returns_false_for_unknown_repo() {
    let fixture = TestFixture::new("unknown");
    let repo = fixture.create_repo("repo");
    let registry = RepoRegistry::default();

    assert!(!registry.is_enabled(&repo));
}

struct TestFixture {
    root: std::path::PathBuf,
    paths: GlobalPaths,
}

impl TestFixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "stateful-repo-registry-{name}-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("old fixture root should be removable");
        }
        fs::create_dir_all(&root).expect("fixture root should be creatable");

        let paths = GlobalPaths::new(root.join("home"));
        Self { root, paths }
    }

    fn create_repo(&self, name: &str) -> std::path::PathBuf {
        let repo = self.root.join(name);
        fs::create_dir_all(repo.join(".git")).expect("git directory should be creatable");
        repo
    }
}

impl Drop for TestFixture {
    fn drop(&mut self) {
        if self.root.exists() {
            fs::remove_dir_all(&self.root).expect("fixture root should be removable");
        }
    }
}
