use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActorType {
    Agent,
    Subagent,
    Human,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolVersion {
    #[serde(rename = "stateful.v1")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub protocol_version: ProtocolVersion,
    pub request_id: String,
    pub observed_at: String,
    pub session: SessionIdentity,
    pub workspace: WorkspaceIdentity,
    pub source: SourceRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionIdentity {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub actor_id: String,
    pub actor_type: ActorType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_actor_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceIdentity {
    pub root: String,
    pub workspace_id: String,
    pub repo_id: String,
    pub worktree_id: String,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Hook,
    Mcp,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceType {
    File,
    Directory,
    Test,
    Task,
    Port,
    Migration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetOperation {
    Read,
    Write,
    Delete,
    Rename,
    Move,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Read,
    Search,
    Diff,
    WriteFile,
    EditFile,
    ApplyPatch,
    DeleteFile,
    RenameFile,
    MoveFile,
    Bash,
    ValidationRun,
    ReconcileAck,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub operation: TargetOperation,
    pub resource_type: ResourceType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_path: Option<String>,
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
