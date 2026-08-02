use std::fs;

use stateful_cli::{
    GlobalPaths, RepoRegistry, detect_git_root, disable_repo, enable_repo,
    workspace_id_for_enabled_repo,
};

#[test]
fn enable_repo_registers_only_identity_and_policy_revision() {
    let fixture = TestFixture::new("enable");
    let repo = fixture.create_repo("repo");
    let nested = repo.join("src/app");
    fs::create_dir_all(&nested).expect("nested repo directory should be creatable");

    let first = enable_repo(&fixture.paths, &nested).expect("repo should enable");
    let canonical_repo = repo.canonicalize().expect("repo should canonicalize");
    assert_eq!(first.root, canonical_repo);
    assert_eq!(first.policy_revision, 1);
    assert!(first.enabled);
    assert_eq!(
        detect_git_root(&nested).expect("git root should be detected"),
        canonical_repo
    );
    assert!(repo.join(".stateful/config.yml").is_file());

    let second = enable_repo(&fixture.paths, &repo).expect("repo should re-enable");
    assert_eq!(second.repo_id, first.repo_id);
    assert_eq!(second.policy_revision, 2);

    let registry = RepoRegistry::load(&fixture.paths).expect("registry should load");
    assert_eq!(registry.repos, vec![second]);
    let saved = fs::read_to_string(&fixture.paths.config_yml).expect("registry should exist");
    assert!(!saved.contains("allowed_tools"));
    assert!(!saved.contains("unclassified_tools"));
}

#[test]
fn registry_rejects_removed_name_only_writer_policy() {
    let fixture = TestFixture::new("legacy-writer-policy");
    fs::create_dir_all(
        fixture
            .paths
            .config_yml
            .parent()
            .expect("config should have a parent"),
    )
    .expect("config parent should be creatable");
    fs::write(
        &fixture.paths.config_yml,
        "repos:\n  - repo_id: repo-1\n    root: /tmp/repo\n    enabled: true\n    enabled_at: '1'\n    policy_config_path: /tmp/repo/.stateful/config.yml\n    allowed_tools: [arbitrary_writer]\n",
    )
    .expect("legacy registry should be writable");

    let error = RepoRegistry::load(&fixture.paths)
        .expect_err("removed writer policy must fail closed")
        .to_string();
    assert!(error.contains("failed to parse repo registry config"));
}

#[test]
fn disable_repo_marks_registered_identity_disabled() {
    let fixture = TestFixture::new("disable");
    let repo = fixture.create_repo("repo");
    let enabled = enable_repo(&fixture.paths, &repo).expect("repo should enable");

    let disabled = disable_repo(&fixture.paths, &repo).expect("repo should disable");
    assert_eq!(disabled.repo_id, enabled.repo_id);
    assert!(!disabled.enabled);
    assert!(
        !RepoRegistry::load(&fixture.paths)
            .expect("registry should load")
            .is_enabled(&repo)
    );

    let metadata = fs::read_to_string(
        fixture
            .paths
            .repos_dir
            .join(format!("{}.json", enabled.repo_id)),
    )
    .expect("metadata should exist");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&metadata).expect("metadata should be json")["enabled"],
        false
    );
}

#[test]
fn workspace_identity_is_stable_and_worktree_scoped() {
    let fixture = TestFixture::new("workspace");
    let repo_a = fixture.create_repo("repo-a");
    let repo_b = fixture.create_repo("repo-b");
    enable_repo(&fixture.paths, &repo_a).expect("repo a should enable");
    enable_repo(&fixture.paths, &repo_b).expect("repo b should enable");

    let a = workspace_id_for_enabled_repo(&fixture.paths, &repo_a)
        .expect("repo a should have a workspace id");
    let a_again = workspace_id_for_enabled_repo(&fixture.paths, &repo_a)
        .expect("repo a identity should be stable");
    let b = workspace_id_for_enabled_repo(&fixture.paths, &repo_b)
        .expect("repo b should have a workspace id");

    assert_eq!(a, a_again);
    assert_ne!(a, b);
    assert!(a.starts_with("workspace-"));
}

struct TestFixture {
    root: tempfile::TempDir,
    paths: GlobalPaths,
}

impl TestFixture {
    fn new(name: &str) -> Self {
        let root = tempfile::Builder::new()
            .prefix(&format!("stateful-repo-registry-{name}-"))
            .tempdir()
            .expect("temp dir should create");
        let paths = GlobalPaths::new(root.path().join("home"));
        Self { root, paths }
    }

    fn create_repo(&self, name: &str) -> std::path::PathBuf {
        let repo = self.root.path().join(name);
        fs::create_dir_all(repo.join(".git")).expect("git directory should be creatable");
        repo
    }
}
