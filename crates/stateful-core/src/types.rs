use crate::V2Error;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActorType {
    Agent,
    Subagent,
    Human,
    System,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolVersion {
    #[serde(rename = "stateful.v2")]
    V2,
}

impl fmt::Display for ProtocolVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::V2 => formatter.write_str("stateful.v2"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentIdentity {
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub actor_id: String,
    pub actor_type: ActorType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_actor_id: Option<String>,
}

impl AgentIdentity {
    pub fn validate(&self) -> Result<(), V2Error> {
        validate_nonempty("agent_id", &self.agent_id)?;
        validate_nonempty("actor_id", &self.actor_id)?;
        for (field, value) in [
            ("turn_id", self.turn_id.as_deref()),
            ("owner_id", self.owner_id.as_deref()),
            ("parent_agent_id", self.parent_agent_id.as_deref()),
            ("parent_actor_id", self.parent_actor_id.as_deref()),
        ] {
            if let Some(value) = value {
                validate_nonempty(field, value)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceIdentity {
    pub root: String,
    pub workspace_id: String,
    pub repo_id: String,
    pub worktree_id: String,
    pub branch: String,
}

impl WorkspaceIdentity {
    pub fn validate(&self) -> Result<(), V2Error> {
        for (field, value) in [
            ("root", &self.root),
            ("workspace_id", &self.workspace_id),
            ("repo_id", &self.repo_id),
            ("worktree_id", &self.worktree_id),
            ("branch", &self.branch),
        ] {
            validate_nonempty(field, value)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Hook,
    Cli,
    Watcher,
    Ide,
    Server,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    pub kind: SourceKind,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    pub source_ref: String,
}

impl SourceRef {
    pub fn validate(&self) -> Result<(), V2Error> {
        validate_nonempty("event", &self.event)?;
        validate_nonempty("source_ref", &self.source_ref)?;
        if let Some(tool_name) = &self.tool_name {
            validate_nonempty("tool_name", tool_name)?;
        }
        Ok(())
    }
}

fn validate_nonempty(field: &str, value: &str) -> Result<(), V2Error> {
    if value.trim().is_empty() {
        return Err(V2Error::new(
            "invalid_identity",
            format!("{field} must not be empty."),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionKind {
    Allow,
    Warn,
    Deny,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub decision: DecisionKind,
    pub reason_code: String,
    pub message: String,
    pub required_next_action: Option<String>,
}

impl Decision {
    pub fn allow(reason_code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            decision: DecisionKind::Allow,
            reason_code: reason_code.into(),
            message: message.into(),
            required_next_action: None,
        }
    }

    pub fn deny(
        reason_code: impl Into<String>,
        message: impl Into<String>,
        required_next_action: impl Into<String>,
    ) -> Self {
        Self {
            decision: DecisionKind::Deny,
            reason_code: reason_code.into(),
            message: message.into(),
            required_next_action: Some(required_next_action.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_kind_accepts_hook_protocol_name() {
        let kind: SourceKind =
            serde_json::from_str(r#""hook""#).expect("hook source kind should deserialize");

        assert_eq!(kind, SourceKind::Hook);
    }
}
