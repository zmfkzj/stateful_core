use stateful_core::{
    DecisionKind, FreshnessMode, ObservationFreshness, ThinSafetyState, evaluate_thin_safety,
};

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
