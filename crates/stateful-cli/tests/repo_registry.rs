use std::fs;

use stateful_cli::{GlobalPaths, RepoRegistry, detect_git_root, disable_repo, enable_repo};

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
    assert_eq!(registry.repos, vec![entry.clone()]);
    assert!(registry.is_enabled(&repo));
    assert_eq!(
        detect_git_root(&nested).expect("git root should be detected"),
        canonical_repo
    );

    assert!(repo.join(".stateful/config.yml").is_file());
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
