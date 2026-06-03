use std::{
    fs,
    path::{Path, PathBuf},
};

use stateful_cli::{
    GlobalPaths, InstallOptions, RepoRegistry, apply_global_install, plan_global_install,
};

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
    assert!(plan.files.contains(&fixture.codex_config));
    assert!(!fixture.paths.home.exists());
    assert!(!fixture.codex_config.exists());
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
    assert!(fixture.codex_config.is_file());
    assert!(!fixture.codex_config.parent().unwrap().join("hooks.json").exists());

    let registry = RepoRegistry::load(&fixture.paths).expect("registry should load");
    assert_eq!(registry, RepoRegistry::default());

    let store = stateful_store::Store::open(&fixture.paths.state_db).expect("store should open");
    assert_eq!(store.event_count().expect("event count should load"), 0);

    let first_config = fs::read_to_string(&fixture.codex_config).expect("codex config should read");
    assert!(first_config.contains("# stateful-core-global-install"));
    assert!(first_config.contains("[mcp_servers.stateful]"));
    assert!(first_config.contains("command = \"/opt/stateful/bin/stateful\""));
    assert!(first_config.contains("hook pre-tool-use"));

    apply_global_install(fixture.options(true)).expect("install should be idempotent");

    let second_config =
        fs::read_to_string(&fixture.codex_config).expect("codex config should reread");
    assert_eq!(count(&second_config, "# stateful-core-global-install"), 1);
    assert_eq!(count(&second_config, "[mcp_servers.stateful]"), 1);
    assert_eq!(count(&second_config, "[[hooks.PreToolUse]]"), 1);
}

#[test]
fn install_yes_backs_up_existing_codex_config_before_merge() {
    let fixture = TestFixture::new("backup");
    let existing = "[tools]\ncustom = true\n";
    fs::create_dir_all(fixture.codex_config.parent().unwrap()).expect("codex dir should create");
    fs::write(&fixture.codex_config, existing).expect("existing config should write");

    apply_global_install(fixture.options(true)).expect("install should apply");

    let merged = fs::read_to_string(&fixture.codex_config).expect("merged config should read");
    assert!(merged.contains(existing));
    assert!(merged.contains("# stateful-core-global-install"));

    let backup = single_backup_for(&fixture.codex_config);
    let backup_contents = fs::read_to_string(backup).expect("backup should read");
    assert_eq!(backup_contents, existing);
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

fn single_backup_for(config_path: &Path) -> PathBuf {
    let parent = config_path.parent().expect("config path should have parent");
    let file_name = config_path
        .file_name()
        .and_then(|name| name.to_str())
        .expect("config file name should be utf-8");
    let prefix = format!("{file_name}.stateful-backup-");
    let backups: Vec<PathBuf> = fs::read_dir(parent)
        .expect("codex config dir should read")
        .map(|entry| entry.expect("dir entry should read").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .collect();

    assert_eq!(backups.len(), 1, "expected one backup, got {backups:?}");
    backups.into_iter().next().expect("backup should exist")
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
            codex_config_path: self.codex_config.clone(),
            binary_path: "/opt/stateful/bin/stateful".to_string(),
        }
    }
}

impl Drop for TestFixture {
    fn drop(&mut self) {
        if self.root.exists() {
            fs::remove_dir_all(&self.root).expect("fixture root should be removable");
        }
    }
}
