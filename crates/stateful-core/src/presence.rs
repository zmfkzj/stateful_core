use crate::{ActorType, PresencePhase, V2Error, normalize_relative_path};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

pub const PRESENCE_TTL: Duration = Duration::minutes(15);
pub const BUSY_UNTIL_MAXIMUM: Duration = Duration::minutes(60);
pub const READ_OBSERVATION_TTL: Duration = Duration::minutes(60);
pub const EXPLICIT_HANDOFF_RELEVANCE: Duration = Duration::days(7);
pub const FALLBACK_HANDOFF_RELEVANCE: Duration = Duration::days(1);
pub const PRESENCE_GOAL_EXCERPT_MAX_SCALARS: usize = 240;
pub const LAST_RESULT_MAX_SCALARS: usize = 240;
pub const HANDOFF_SUMMARY_MAX_SCALARS: usize = 2_000;
pub const HANDOFF_LIST_MAX_ENTRIES: usize = 100;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_excerpt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<PresencePhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_result: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub busy_until: Option<OffsetDateTime>,
}

impl PresenceUpdate {
    pub fn normalized(mut self) -> Result<Self, V2Error> {
        if let Some(goal_excerpt) = &self.goal_excerpt {
            let normalized = normalize_whitespace(goal_excerpt);
            validate_scalar_limit(
                "goal_excerpt",
                &normalized,
                PRESENCE_GOAL_EXCERPT_MAX_SCALARS,
            )?;
            self.goal_excerpt = Some(normalized);
        }
        if let Some(last_result) = &self.last_result {
            validate_scalar_limit("last_result", last_result, LAST_RESULT_MAX_SCALARS)?;
        }
        Ok(self)
    }

    pub fn validate_busy_until(&self, tool_started_at: OffsetDateTime) -> Result<(), V2Error> {
        if let Some(busy_until) = self.busy_until
            && busy_until > tool_started_at + BUSY_UNTIL_MAXIMUM
        {
            return Err(V2Error::new(
                "busy_until_too_far",
                "busy_until must not be more than 60 minutes after tool start.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceRecord {
    pub workspace_id: String,
    pub agent_id: String,
    pub actor_id: String,
    pub actor_type: ActorType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_actor_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_excerpt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<PresencePhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_result: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub registered_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub busy_until: Option<OffsetDateTime>,
    pub origin_event_seq: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PresenceResourceRelation {
    Read,
    Planned,
    Touched,
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceResource {
    pub workspace_id: String,
    pub agent_id: String,
    pub relative_path: String,
    pub relation: PresenceResourceRelation,
    #[serde(with = "time::serde::rfc3339")]
    pub observed_at: OffsetDateTime,
    pub origin_event_seq: u64,
}

impl PresenceResource {
    pub fn new(
        workspace_id: impl Into<String>,
        agent_id: impl Into<String>,
        relative_path: impl Into<String>,
        relation: PresenceResourceRelation,
        observed_at: OffsetDateTime,
        origin_event_seq: u64,
    ) -> Result<Self, V2Error> {
        let workspace_id = workspace_id.into();
        let agent_id = agent_id.into();
        let relative_path = relative_path.into();
        validate_required("workspace_id", &workspace_id)?;
        validate_required("agent_id", &agent_id)?;
        let normalized = normalize_relative_path(&relative_path);
        if normalized.is_empty() || normalized != relative_path {
            return Err(V2Error::new(
                "invalid_relative_path",
                "relative_path must be a normalized nonempty relative path.",
            ));
        }
        Ok(Self {
            workspace_id,
            agent_id,
            relative_path,
            relation,
            observed_at,
            origin_event_seq,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HandoffStatus {
    Done,
    Failed,
    Blocked,
    Cancelled,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplicitHandoff {
    pub status: HandoffStatus,
    pub summary: String,
    #[serde(default)]
    pub files_changed: Vec<String>,
    #[serde(default)]
    pub tests_run: Vec<String>,
    #[serde(default)]
    pub remaining_work: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_plan: Option<String>,
}

impl Default for ExplicitHandoff {
    fn default() -> Self {
        Self {
            status: HandoffStatus::Unknown,
            summary: String::new(),
            files_changed: Vec::new(),
            tests_run: Vec::new(),
            remaining_work: Vec::new(),
            next_plan: None,
        }
    }
}

impl ExplicitHandoff {
    pub fn validate(&self) -> Result<(), V2Error> {
        validate_required("summary", &self.summary)?;
        validate_scalar_limit(
            "handoff_summary",
            &self.summary,
            HANDOFF_SUMMARY_MAX_SCALARS,
        )?;
        for (field, entries) in [
            ("files_changed", &self.files_changed),
            ("tests_run", &self.tests_run),
            ("remaining_work", &self.remaining_work),
        ] {
            if entries.len() > HANDOFF_LIST_MAX_ENTRIES {
                return Err(V2Error::new(
                    "handoff_list_too_long",
                    format!("{field} must contain at most {HANDOFF_LIST_MAX_ENTRIES} entries."),
                ));
            }
        }
        for relative_path in &self.files_changed {
            let normalized = normalize_relative_path(relative_path);
            if normalized.is_empty() || normalized != *relative_path {
                return Err(V2Error::new(
                    "invalid_relative_path",
                    "files_changed entries must be normalized nonempty relative paths.",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffRecord {
    pub workspace_id: String,
    pub agent_id: String,
    pub actor_id: String,
    pub actor_type: ActorType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_actor_id: Option<String>,
    pub status: HandoffStatus,
    pub summary: String,
    #[serde(default)]
    pub files_changed: Vec<String>,
    #[serde(default)]
    pub tests_run: Vec<String>,
    #[serde(default)]
    pub remaining_work: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_result: Option<String>,
    pub explicit: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub finalized_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub origin_event_seq: u64,
}

fn normalize_whitespace(value: &str) -> String {
    let mut normalized = String::new();
    for part in value.split_whitespace() {
        if !normalized.is_empty() {
            normalized.push(' ');
        }
        normalized.push_str(part);
    }
    normalized
}

fn validate_scalar_limit(field: &str, value: &str, maximum: usize) -> Result<(), V2Error> {
    if value.chars().count() > maximum {
        return Err(V2Error::new(
            format!("{field}_too_long"),
            format!("{field} must contain at most {maximum} Unicode scalar values."),
        ));
    }
    Ok(())
}

fn validate_required(field: &str, value: &str) -> Result<(), V2Error> {
    if value.trim().is_empty() {
        return Err(V2Error::new(
            format!("invalid_{field}"),
            format!("{field} must not be empty."),
        ));
    }
    Ok(())
}
