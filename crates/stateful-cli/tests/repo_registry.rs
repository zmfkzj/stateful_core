use std::fs;

use stateful_cli::{
    GlobalPaths, RepoRegistry, allow_tool_for_repo, deny_tool_for_repo, detect_git_root,
    disable_repo, enable_repo, record_unclassified_tool_for_repo, tool_allowed_for_enabled_repo,
    tool_list_for_repo, workspace_id_for_enabled_repo,
};

fn default_allowed_tools() -> Vec<String> {
    [
        "multi_agent_v1spawn_agent",
        "multi_agent_v1wait_agent",
        "multi_agent_v1close_agent",
        "multi_agent_v1resume_agent",
        "mcp__openaiDeveloperDocs__fetch_openai_doc",
        "mcp__openaiDeveloperDocs__search_openai_docs",
        "multi_agent_v1send_input",
        "task",
        "yield",
        "parallel_tool_calls",
        "lsp",
        "glob",
        "ask",
        "ast_grep",
        "browser",
        "find",
        "generate_image",
        "grep",
        "irc",
        "job",
        "read",
        "report_tool_issue",
        "search",
        "search_tool_bm25",
        "todo",
        "web_search",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

#[test]
fn enable_repo_registers_git_root_and_writes_repo_configs() {
    let fixture = TestFixture::new("enable");
    let repo = fixture.create_repo("repo");
    let nested = repo.join("src").join("app");
    fs::create_dir_all(&nested).expect("nested repo directory should be creatable");

    let entry = enable_repo(&fixture.paths, &nested).expect("repo should enable");
    let registry = RepoRegistry::load(&fixture.paths).expect("registry should load");
    let canonical_repo = repo
        .canonicalize()
        .expect("repo root should canonicalize after creation");

    assert_eq!(entry.root, canonical_repo);
    assert_eq!(
        entry.policy_config_path,
        canonical_repo.join(".stateful/config.yml")
    );
    assert!(entry.enabled);
    assert!(!entry.enabled_at.is_empty());
    assert_eq!(entry.allowed_tools, default_allowed_tools());
    assert_eq!(registry.repos, vec![entry.clone()]);
    assert!(registry.is_enabled(&repo));
    assert_eq!(
        detect_git_root(&nested).expect("git root should be detected"),
        canonical_repo
    );

    assert!(repo.join(".stateful/config.yml").is_file());
    let repo_config = fs::read_to_string(repo.join(".stateful/config.yml"))
        .expect("repo config yml should exist");
    assert!(repo_config.contains("informational target defaults"));
    assert!(repo_config.contains("Runtime loading of these keys is not yet shipped."));
    assert!(!repo.join(".stateful/validation.yml").exists());
    assert!(
        fixture
            .paths
            .repos_dir
            .join(format!("{}.json", entry.repo_id))
            .is_file()
    );

    let saved = fs::read_to_string(&fixture.paths.config_yml).expect("registry yml should exist");
    assert!(!saved.contains("codex_mode"));

    let replaced = enable_repo(&fixture.paths, &repo).expect("repo should re-enable");
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

    let enabled = enable_repo(&fixture.paths, &repo).expect("repo should enable");
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

    let enabled = enable_repo(&fixture.paths, &repo).expect("repo should enable");
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
fn enabled_lookup_returns_false_for_unknown_repo() {
    let fixture = TestFixture::new("unknown");
    let repo = fixture.create_repo("repo");
    let registry = RepoRegistry::default();

    assert!(!registry.is_enabled(&repo));
}

#[test]
fn workspace_id_is_stable_for_enabled_repo_and_distinct_per_root() {
    let fixture = TestFixture::new("workspace-id");
    let repo_a = fixture.create_repo("repo-a");
    let repo_b = fixture.create_repo("repo-b");
    let nested_a = repo_a.join("src");
    fs::create_dir_all(&nested_a).expect("nested repo directory should be creatable");

    enable_repo(&fixture.paths, &repo_a).expect("repo a should enable");
    enable_repo(&fixture.paths, &repo_b).expect("repo b should enable");

    let workspace_a = workspace_id_for_enabled_repo(&fixture.paths, &nested_a)
        .expect("enabled repo a should have a workspace id");
    let workspace_a_again = workspace_id_for_enabled_repo(&fixture.paths, &repo_a)
        .expect("workspace id should be stable");
    let workspace_b = workspace_id_for_enabled_repo(&fixture.paths, &repo_b)
        .expect("enabled repo b should have a workspace id");

    assert_eq!(workspace_a, workspace_a_again);
    assert_ne!(workspace_a, workspace_b);
    assert!(workspace_a.starts_with("workspace-"));
    assert_ne!(workspace_a, "local");
    assert_ne!(workspace_b, "local");
}

#[test]
fn tool_allowlist_is_repo_scoped_deduplicated_and_preserved() {
    let fixture = TestFixture::new("tool-allowlist");
    let repo_a = fixture.create_repo("repo-a");
    let repo_b = fixture.create_repo("repo-b");
    let nested_a = repo_a.join("src");
    fs::create_dir_all(&nested_a).expect("nested repo directory should be creatable");

    enable_repo(&fixture.paths, &repo_a).expect("repo a should enable");
    enable_repo(&fixture.paths, &repo_b).expect("repo b should enable");

    assert!(
        !tool_allowed_for_enabled_repo(&fixture.paths, &nested_a, "FutureWriteTool")
            .expect("allow lookup should work before allow")
    );

    let entry = allow_tool_for_repo(&fixture.paths, &nested_a, "FutureWriteTool")
        .expect("tool should be allowed for repo a");
    let mut expected_repo_a_tools = default_allowed_tools();
    expected_repo_a_tools.push("FutureWriteTool".to_string());
    assert_eq!(entry.allowed_tools, expected_repo_a_tools);

    allow_tool_for_repo(&fixture.paths, &repo_a, "FutureWriteTool")
        .expect("duplicate allow should be idempotent");
    let registry = RepoRegistry::load(&fixture.paths).expect("registry should reload");
    let repo_a_entry = registry
        .repos
        .iter()
        .find(|entry| entry.root == repo_a.canonicalize().expect("repo a should canonicalize"))
        .expect("repo a should be registered");
    assert_eq!(repo_a_entry.allowed_tools, expected_repo_a_tools);

    assert!(
        tool_allowed_for_enabled_repo(&fixture.paths, &nested_a, "FutureWriteTool")
            .expect("allow lookup should work for repo a")
    );
    assert!(
        !tool_allowed_for_enabled_repo(&fixture.paths, &repo_b, "FutureWriteTool")
            .expect("allow lookup should work for repo b")
    );

    enable_repo(&fixture.paths, &repo_a).expect("re-enable should preserve allowlist");
    assert!(
        tool_allowed_for_enabled_repo(&fixture.paths, &repo_a, "FutureWriteTool")
            .expect("allow lookup should survive re-enable")
    );

    let entry = deny_tool_for_repo(&fixture.paths, &repo_a, "FutureWriteTool")
        .expect("tool should be removed from repo allowlist");
    assert_eq!(entry.allowed_tools, default_allowed_tools());
    assert!(
        !tool_allowed_for_enabled_repo(&fixture.paths, &repo_a, "FutureWriteTool")
            .expect("allow lookup should work after deny")
    );
}

#[test]
fn tool_list_includes_recorded_unclassified_tools() {
    let fixture = TestFixture::new("tool-list-unclassified");
    let repo = fixture.create_repo("repo");
    let nested = repo.join("src");
    fs::create_dir_all(&nested).expect("nested repo directory should be creatable");
    enable_repo(&fixture.paths, &repo).expect("repo should enable");

    allow_tool_for_repo(&fixture.paths, &repo, "KnownTool").expect("known tool should be allowed");
    record_unclassified_tool_for_repo(&fixture.paths, &nested, "FutureWriteTool")
        .expect("unclassified tool should be recorded");
    record_unclassified_tool_for_repo(&fixture.paths, &repo, "FutureWriteTool")
        .expect("duplicate unclassified record should be idempotent");

    let list = tool_list_for_repo(&fixture.paths, &repo).expect("tool list should load");
    let mut expected_allowed_tools = default_allowed_tools();
    expected_allowed_tools.push("KnownTool".to_string());
    assert_eq!(list.allowed_tools, expected_allowed_tools);
    assert_eq!(list.unclassified_tools, vec!["FutureWriteTool"]);

    allow_tool_for_repo(&fixture.paths, &repo, "FutureWriteTool")
        .expect("allowing a tool should remove it from unclassified tools");
    let list = tool_list_for_repo(&fixture.paths, &repo).expect("tool list should reload");
    expected_allowed_tools.push("FutureWriteTool".to_string());
    assert_eq!(list.allowed_tools, expected_allowed_tools);
    assert!(list.unclassified_tools.is_empty());
}

#[test]
fn tool_allowlist_rejects_empty_or_control_character_tool_names() {
    let fixture = TestFixture::new("tool-allowlist-invalid");
    let repo = fixture.create_repo("repo");
    enable_repo(&fixture.paths, &repo).expect("repo should enable");

    let empty = allow_tool_for_repo(&fixture.paths, &repo, "  ")
        .expect_err("empty tool name should be rejected")
        .to_string();
    assert!(empty.contains("tool name must not be empty"));

    let control = allow_tool_for_repo(&fixture.paths, &repo, "tool\nname")
        .expect_err("control character tool name should be rejected")
        .to_string();
    assert!(control.contains("tool name must not contain control characters"));
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
