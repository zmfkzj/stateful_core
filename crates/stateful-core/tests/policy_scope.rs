use stateful_core::{
    ReservationScope, ScopeSet, normalize_relative_path, normalized_relative_path_is_empty,
};

#[test]
fn directory_scope_does_not_allow_file_writes() {
    let scope = ReservationScope::directory("src/");

    assert!(!scope.allows_write("src/auth.ts"));
    assert!(!scope.allows_write("src/auth/nested.ts"));
}

#[test]
fn write_directory_requires_exact_directory_scope() {
    let scope = ReservationScope::directory("target/");

    assert!(scope.allows_write_directory("target/"));
    assert!(scope.allows_write_directory("target"));
    assert!(!scope.allows_write_directory("target/debug/"));
    assert!(!scope.allows_write_directory("other/"));
}

#[test]
fn file_scope_allows_only_exact_file_write() {
    let scope = ReservationScope::file("src/auth.ts");

    assert!(scope.allows_write("src/auth.ts"));
    assert!(!scope.allows_write("src/session.ts"));
}

#[test]
fn delete_requires_exact_file_scope() {
    let directory = ReservationScope::directory("src/");
    let file = ReservationScope::file("src/auth.ts");

    assert!(!directory.allows_delete("src/auth.ts"));
    assert!(file.allows_delete("src/auth.ts"));
}

#[test]
fn rename_requires_exact_source_and_destination_file_scope() {
    let scopes = ScopeSet::new(vec![
        ReservationScope::file("src/old.ts"),
        ReservationScope::file("src/new.ts"),
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
