use stateful_core::{
    ExplicitHandoff, HANDOFF_LIST_MAX_ENTRIES, HANDOFF_SUMMARY_MAX_SCALARS, HandoffStatus,
    PRESENCE_GOAL_EXCERPT_MAX_SCALARS, PresenceUpdate,
};

#[test]
fn goal_excerpt_normalizes_whitespace_and_counts_unicode_scalars() {
    let update = PresenceUpdate {
        goal_excerpt: Some("fix  auth\n flow".into()),
        ..Default::default()
    };
    let normalized = update.normalized().expect("short normalized goal is valid");
    assert_eq!(normalized.goal_excerpt.as_deref(), Some("fix auth flow"));

    let boundary = PresenceUpdate {
        goal_excerpt: Some("🦀".repeat(PRESENCE_GOAL_EXCERPT_MAX_SCALARS)),
        ..Default::default()
    };
    assert!(boundary.normalized().is_ok());

    let over_limit = PresenceUpdate {
        goal_excerpt: Some("🦀".repeat(PRESENCE_GOAL_EXCERPT_MAX_SCALARS + 1)),
        ..Default::default()
    };
    let error = over_limit
        .normalized()
        .expect_err("over-limit goal must fail");
    assert_eq!(error.code, "goal_excerpt_too_long");
}

#[test]
fn handoff_limits_reject_instead_of_truncating() {
    let handoff = ExplicitHandoff {
        status: HandoffStatus::Done,
        summary: "completed".into(),
        files_changed: (0..=HANDOFF_LIST_MAX_ENTRIES)
            .map(|index| format!("src/{index}.rs"))
            .collect(),
        ..Default::default()
    };
    let error = handoff.validate().expect_err("over-limit files must fail");
    assert_eq!(error.code, "handoff_list_too_long");
    assert_eq!(handoff.files_changed.len(), HANDOFF_LIST_MAX_ENTRIES + 1);

    let long_summary = ExplicitHandoff {
        status: HandoffStatus::Done,
        summary: "🦀".repeat(HANDOFF_SUMMARY_MAX_SCALARS + 1),
        ..Default::default()
    };
    let error = long_summary
        .validate()
        .expect_err("over-limit summary must fail");
    assert_eq!(error.code, "handoff_summary_too_long");

    let unnormalized_path = ExplicitHandoff {
        status: HandoffStatus::Done,
        summary: "completed".into(),
        files_changed: vec!["src//lib.rs".into()],
        ..Default::default()
    };
    let error = unnormalized_path
        .validate()
        .expect_err("handoff paths must be normalized");
    assert_eq!(error.code, "invalid_relative_path");
}
