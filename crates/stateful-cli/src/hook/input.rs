use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct CodexSessionStart {
    #[serde(alias = "session_id")]
    pub(crate) agent_id: String,
    #[serde(default)]
    pub(crate) prompt: Option<String>,
}

impl CodexSessionStart {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        crate::validate_agent_id(&self.agent_id, "agent_id")
    }

    pub(crate) fn first_prompt(&self) -> Option<&str> {
        self.prompt
            .as_deref()
            .map(str::trim)
            .filter(|prompt| !prompt.is_empty())
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct CodexUserPrompt {
    #[serde(alias = "session_id")]
    pub(crate) agent_id: String,
    #[serde(default)]
    prompt: String,
}

impl CodexUserPrompt {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        crate::validate_agent_id(&self.agent_id, "agent_id")
    }

    pub(crate) fn prompt(&self) -> Option<&str> {
        (!self.prompt.trim().is_empty()).then_some(self.prompt.trim())
    }
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ToolMetadata {
    #[serde(default)]
    tool_use_id: Option<String>,
    #[serde(default)]
    tool_call_id: Option<String>,
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    operation_id: Option<String>,
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    complete: Option<bool>,
    #[serde(default)]
    is_complete: Option<bool>,
    #[serde(default)]
    truncated: bool,
    #[serde(default)]
    is_truncated: bool,
    #[serde(default)]
    result_summary: Option<String>,
}

impl ToolMetadata {
    pub(crate) fn operation_id(&self) -> Option<&str> {
        [
            self.tool_use_id.as_deref(),
            self.tool_call_id.as_deref(),
            self.call_id.as_deref(),
            self.operation_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
    }

    pub(crate) fn successful(&self) -> bool {
        !self.is_error && self.success != Some(false)
    }

    pub(crate) fn failed(&self) -> bool {
        !self.successful()
    }

    pub(crate) fn complete(&self) -> bool {
        self.is_complete.or(self.complete).unwrap_or(true)
    }

    pub(crate) fn truncated(&self) -> bool {
        self.truncated || self.is_truncated
    }

    pub(crate) fn result_summary(&self) -> Option<&str> {
        self.result_summary
            .as_deref()
            .map(str::trim)
            .filter(|summary| !summary.is_empty())
    }
}
