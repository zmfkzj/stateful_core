use crate::{ContentFingerprint, Decision, DecisionKind};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

pub const OBSERVATION_TTL: Duration = Duration::minutes(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadClassification {
    Exact,
    StructuralSummary,
    Partial,
    Truncated,
    Failed,
    Ambiguous,
}

impl ReadClassification {
    pub const fn may_stabilize(self) -> bool {
        matches!(self, Self::Exact | Self::StructuralSummary)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadObservationStatus {
    Started,
    Stabilized,
    Unstable,
    Aborted,
    Invalidated,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadObservationStart {
    pub operation_id: String,
    pub path: String,
    pub before: ContentFingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadCompletion {
    pub operation_id: String,
    pub path: String,
    pub classification: ReadClassification,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<ContentFingerprint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_marker: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadObservationRecord {
    pub workspace_id: String,
    pub agent_id: String,
    pub operation_id: String,
    pub path: String,
    pub status: ReadObservationStatus,
    pub classification: ReadClassification,
    pub before: ContentFingerprint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<ContentFingerprint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_marker: Option<String>,
    pub observed_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<OffsetDateTime>,
    pub resource_version: u64,
    #[serde(default)]
    pub origin_event_seq: u64,
}

impl ReadObservationRecord {
    pub fn is_stable(&self) -> bool {
        self.status == ReadObservationStatus::Stabilized
    }

    pub fn is_fresh_at(&self, now: OffsetDateTime) -> bool {
        self.is_stable() && self.expires_at.is_some_and(|expires_at| expires_at >= now)
    }
}

pub fn observation_status(
    classification: ReadClassification,
    before: &ContentFingerprint,
    after: Option<&ContentFingerprint>,
    semantic_marker: Option<&str>,
) -> ReadObservationStatus {
    match classification {
        ReadClassification::Failed => ReadObservationStatus::Aborted,
        ReadClassification::Exact => match after {
            Some(after) if before == after && before.is_complete_exact() && after.is_complete_exact() => {
                ReadObservationStatus::Stabilized
            }
            _ => ReadObservationStatus::Unstable,
        },
        ReadClassification::StructuralSummary => {
            if semantic_marker.is_some_and(|marker| !marker.trim().is_empty()) {
                ReadObservationStatus::Stabilized
            } else {
                ReadObservationStatus::Unstable
            }
        }
        ReadClassification::Partial | ReadClassification::Truncated | ReadClassification::Ambiguous => {
            ReadObservationStatus::Unstable
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteTarget {
    pub path: String,
    pub before: ContentFingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteIntentStart {
    pub operation_id: String,
    pub action: String,
    pub targets: Vec<WriteTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteIntentCompletion {
    pub intent_id: String,
    pub outcome: WriteIntentOutcome,
    #[serde(default)]
    pub post_fingerprints: Vec<(String, ContentFingerprint)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
}

impl WriteIntentCompletion {
    pub fn committed(intent_id: String, post_fingerprints: Vec<(String, ContentFingerprint)>) -> Self {
        Self {
            intent_id,
            outcome: WriteIntentOutcome::Committed,
            post_fingerprints,
            failure_code: None,
        }
    }

    pub fn failed(intent_id: String, failure_code: impl Into<String>) -> Self {
        Self {
            intent_id,
            outcome: WriteIntentOutcome::Failed,
            post_fingerprints: Vec::new(),
            failure_code: Some(failure_code.into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteIntentOutcome {
    Committed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteIntentStatus {
    Started,
    Committed,
    Failed,
    OutcomeUnknown,
    Reconciled,
}

impl WriteIntentStatus {
    pub const fn blocks_writes(self) -> bool {
        matches!(self, Self::Started | Self::OutcomeUnknown)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteIntentRecord {
    pub intent_id: String,
    pub operation_id: String,
    pub workspace_id: String,
    pub agent_id: String,
    pub action: String,
    pub targets: Vec<WriteTarget>,
    pub fence_ids: Vec<String>,
    pub status: WriteIntentStatus,
    pub started_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<OffsetDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
    #[serde(default)]
    pub origin_event_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceVersion {
    pub workspace_id: String,
    pub path: String,
    pub version: u64,
    pub fingerprint: ContentFingerprint,
    pub writer_agent_id: String,
    pub intent_id: String,
    #[serde(default)]
    pub origin_event_seq: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshnessMode {
    Enforcement,
    Awareness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationFreshness {
    Stable,
    Missing,
    Expired,
    Changed,
    Unstable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThinSafetyState {
    pub invalid_target: bool,
    pub unknown_write_outcome: bool,
    pub observation: ObservationFreshness,
    pub active_fence: bool,
    pub unreconciled_human_write: bool,
}

pub fn evaluate_thin_safety(state: ThinSafetyState, mode: FreshnessMode) -> Decision {
    if state.invalid_target {
        return Decision::deny(
            "invalid_target",
            "Write target is invalid or violates the security boundary.",
            "Use a normalized workspace-relative target within the workspace root.",
        );
    }
    if state.unknown_write_outcome {
        return Decision::deny(
            "unknown_write_outcome",
            "A previous write has an unknown changed outcome for this target.",
            "Complete a matching exact reread and reconcile the specific write intent before writing.",
        );
    }
    if matches!(state.observation, ObservationFreshness::Changed | ObservationFreshness::Unstable) {
        return Decision::deny(
            "stale_observation",
            "The supplied read observation is not stable for the current resource version.",
            "Reread the exact target completely and retry with the resulting stable observation.",
        );
    }
    if state.active_fence {
        return Decision::deny(
            "write_fence_conflict",
            "Write target already has an active write fence.",
            "Wait for the in-flight write to complete, then reread before retrying.",
        );
    }
    if state.unreconciled_human_write {
        return Decision::deny(
            "unreconciled_human_write",
            "Write target has an unreconciled high-confidence human change.",
            "Reread the target and reconcile the human change before retrying.",
        );
    }
    match state.observation {
        ObservationFreshness::Stable => Decision::allow(
            "stable_observation",
            "A stable read observation matches the current resource version.",
        ),
        ObservationFreshness::Missing | ObservationFreshness::Expired => match mode {
            FreshnessMode::Enforcement => Decision::deny(
                "missing_read_provenance",
                "No fresh stable read observation covers this write target.",
                "Perform a complete exact read of the target before writing.",
            ),
            FreshnessMode::Awareness => Decision {
                decision: DecisionKind::Warn,
                reason_code: "missing_read_provenance".into(),
                message: "No fresh stable read observation covers this write target.".into(),
                required_next_action: Some(
                    "Warned in awareness mode: perform a complete exact read before writing when possible."
                        .into(),
                ),
            },
        },
        ObservationFreshness::Changed | ObservationFreshness::Unstable => unreachable!("handled before soft policy"),
    }
}
