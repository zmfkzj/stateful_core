use stateful_core::{AuthorizationInput, DecisionKind, PolicyState, authorize_action};

#[test]
fn write_without_active_intent_is_denied() {
    let state = PolicyState::default();
    let input = AuthorizationInput::write_file("src/auth.ts");

    let decision = authorize_action(&state, input);

    assert_eq!(decision.decision, DecisionKind::Deny);
    assert_eq!(decision.reason_code, "missing_intent");
}

#[test]
fn matching_file_intent_allows_write() {
    let state = PolicyState::default().with_active_file_intent("src/auth.ts");
    let input = AuthorizationInput::write_file("src/auth.ts");

    let decision = authorize_action(&state, input);

    assert_eq!(decision.decision, DecisionKind::Allow);
}

#[test]
fn write_file_requires_exact_file_intent_in_authorization_engine() {
    let directory = PolicyState::default().with_active_directory_intent("src/");
    let file = PolicyState::default().with_active_file_intent("src/auth.ts");

    let denied = authorize_action(&directory, AuthorizationInput::write_file("src/auth.ts"));
    let allowed = authorize_action(&file, AuthorizationInput::write_file("src/auth.ts"));

    assert_eq!(denied.decision, DecisionKind::Deny);
    assert_eq!(denied.reason_code, "scope_mismatch");
    assert_eq!(allowed.decision, DecisionKind::Allow);
}

#[test]
fn matching_directory_intent_allows_write_directory() {
    let state = PolicyState::default().with_active_directory_intent("target/");
    let input = AuthorizationInput::write_directory("target/");

    let decision = authorize_action(&state, input);

    assert_eq!(decision.decision, DecisionKind::Allow);
}

#[test]
fn write_directory_requires_matching_directory_intent() {
    let file = PolicyState::default().with_active_file_intent("target/out.txt");
    let parent = PolicyState::default().with_active_directory_intent("target/");

    let file_decision = authorize_action(&file, AuthorizationInput::write_directory("target/"));
    let nested_decision = authorize_action(
        &parent,
        AuthorizationInput::write_directory("target/debug/"),
    );

    assert_eq!(file_decision.decision, DecisionKind::Deny);
    assert_eq!(file_decision.reason_code, "scope_mismatch");
    assert_eq!(nested_decision.decision, DecisionKind::Deny);
    assert_eq!(nested_decision.reason_code, "scope_mismatch");
}

#[test]
fn target_outside_intent_scope_is_denied() {
    let state = PolicyState::default().with_active_file_intent("src/auth.ts");
    let input = AuthorizationInput::write_file("src/session.ts");

    let decision = authorize_action(&state, input);

    assert_eq!(decision.decision, DecisionKind::Deny);
    assert_eq!(decision.reason_code, "scope_mismatch");
}

#[test]
fn delete_requires_exact_file_intent_in_authorization_engine() {
    let directory = PolicyState::default().with_active_directory_intent("src/");
    let file = PolicyState::default().with_active_file_intent("src/auth.ts");

    let denied = authorize_action(&directory, AuthorizationInput::delete_file("src/auth.ts"));
    let allowed = authorize_action(&file, AuthorizationInput::delete_file("src/auth.ts"));

    assert_eq!(denied.decision, DecisionKind::Deny);
    assert_eq!(denied.reason_code, "scope_mismatch");
    assert_eq!(allowed.decision, DecisionKind::Allow);
}

#[test]
fn rename_requires_exact_source_and_destination_intents_in_authorization_engine() {
    let state = PolicyState::default().with_active_intent_scopes(vec![
        stateful_core::IntentScope::file("src/old.ts"),
        stateful_core::IntentScope::file("src/new.ts"),
    ]);

    let allowed = authorize_action(
        &state,
        AuthorizationInput::rename_file("src/old.ts", "src/new.ts"),
    );
    let denied = authorize_action(
        &state,
        AuthorizationInput::rename_file("src/old.ts", "src/other.ts"),
    );

    assert_eq!(allowed.decision, DecisionKind::Allow);
    assert_eq!(denied.decision, DecisionKind::Deny);
    assert_eq!(denied.reason_code, "scope_mismatch");
}
