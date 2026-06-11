use stateful_core::{
    IntentScope, ScopeSet, normalize_relative_path, normalized_relative_path_is_empty,
};

#[test]
fn directory_scope_allows_depth_two_but_not_depth_three() {
    let scope = IntentScope::directory("src/");

    assert!(scope.allows_write("src/auth/auth.ts"));
    assert!(!scope.allows_write("src/auth/codex/auth.ts"));
}

#[test]
fn write_directory_requires_exact_directory_scope() {
    let scope = IntentScope::directory("target/");

    assert!(scope.allows_write_directory("target/"));
    assert!(scope.allows_write_directory("target"));
    assert!(!scope.allows_write_directory("target/debug/"));
    assert!(!scope.allows_write_directory("other/"));
}

#[test]
fn file_scope_allows_only_exact_file_write() {
    let scope = IntentScope::file("src/auth.ts");

    assert!(scope.allows_write("src/auth.ts"));
    assert!(!scope.allows_write("src/session.ts"));
}

#[test]
fn delete_requires_exact_file_scope() {
    let directory = IntentScope::directory("src/");
    let file = IntentScope::file("src/auth.ts");

    assert!(!directory.allows_delete("src/auth.ts"));
    assert!(file.allows_delete("src/auth.ts"));
}

#[test]
fn rename_requires_exact_source_and_destination_file_scope() {
    let scopes = ScopeSet::new(vec![
        IntentScope::file("src/old.ts"),
        IntentScope::file("src/new.ts"),
    ]);

    assert!(scopes.allows_rename("src/old.ts", "src/new.ts"));
    assert!(!scopes.allows_rename("src/old.ts", "src/other.ts"));
}

#[test]
fn core_normalizes_workspace_relative_paths_for_shared_policy_keys() {
    assert_eq!(
        normalize_relative_path(r"src//auth/../auth.ts"),
        "src/auth.ts"
    );
    assert_eq!(normalize_relative_path(r"src\.\auth.ts"), "src/auth.ts");
    assert!(normalized_relative_path_is_empty("./../"));
    assert!(normalized_relative_path_is_empty(" . "));
}
