use stateful_core::{
    ContentFingerprint, DecisionKind, FreshnessMode, ObservationFreshness, ReadClassification,
    ReadObservationRecord, ReadObservationStatus, ThinSafetyState, evaluate_thin_safety,
};
use time::{Duration, macros::datetime};

fn state(observation: ObservationFreshness) -> ThinSafetyState {
    ThinSafetyState {
        invalid_target: false,
        unknown_write_outcome: false,
        observation,
        active_fence: false,
        unreconciled_human_write: false,
    }
}

#[test]
fn stable_equal_observation_allows_in_both_modes() {
    for mode in [FreshnessMode::Enforcement, FreshnessMode::Awareness] {
        assert_eq!(
            evaluate_thin_safety(state(ObservationFreshness::Stable), mode).decision,
            DecisionKind::Allow
        );
    }
}

#[test]
fn stable_changed_observation_denies_in_both_modes() {
    for mode in [FreshnessMode::Enforcement, FreshnessMode::Awareness] {
        let decision = evaluate_thin_safety(state(ObservationFreshness::Changed), mode);
        assert_eq!(decision.decision, DecisionKind::Deny);
        assert_eq!(decision.reason_code, "stale_observation");
    }
}

#[test]
fn missing_or_expired_observation_warns_in_awareness_and_denies_in_enforcement() {
    for freshness in [ObservationFreshness::Missing, ObservationFreshness::Expired] {
        assert_eq!(
            evaluate_thin_safety(state(freshness), FreshnessMode::Enforcement).decision,
            DecisionKind::Deny
        );
        assert_eq!(
            evaluate_thin_safety(state(freshness), FreshnessMode::Awareness).decision,
            DecisionKind::Warn
        );
    }
}

#[test]
fn thin_hard_stops_remain_denials_in_awareness_before_missing_provenance() {
    for mutate in [
        |value: &mut ThinSafetyState| value.invalid_target = true,
        |value: &mut ThinSafetyState| value.unknown_write_outcome = true,
        |value: &mut ThinSafetyState| value.active_fence = true,
        |value: &mut ThinSafetyState| value.unreconciled_human_write = true,
    ] {
        let mut input = state(ObservationFreshness::Missing);
        mutate(&mut input);
        assert_eq!(
            evaluate_thin_safety(input, FreshnessMode::Awareness).decision,
            DecisionKind::Deny
        );
    }
}

#[test]
fn persisted_structural_observation_is_never_fresh() {
    let now = datetime!(2026-07-16 12:00 UTC);
    let fingerprint = ContentFingerprint::missing();
    let observation = ReadObservationRecord {
        workspace_id: "workspace-1".into(),
        agent_id: "agent-1".into(),
        actor_id: "actor-1".into(),
        operation_id: "read-1".into(),
        path: "src/lib.rs".into(),
        status: ReadObservationStatus::Stabilized,
        classification: ReadClassification::StructuralSummary,
        before: fingerprint.clone(),
        after: Some(fingerprint),
        semantic_marker: Some("legacy-marker".into()),
        observed_at: now,
        expires_at: Some(now + Duration::hours(1)),
        resource_version: 1,
        origin_event_seq: 1,
    };

    assert!(!observation.is_stable());
    assert!(!observation.is_fresh_at(now));
}
