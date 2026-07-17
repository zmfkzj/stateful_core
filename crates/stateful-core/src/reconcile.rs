#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationDecision {
    Adopt,
    Reapply,
    AskUser,
    Abandon,
}

impl ReconciliationDecision {
    pub fn clears_human_write_block(self) -> bool {
        matches!(self, Self::Adopt | Self::Reapply)
    }
}

impl std::str::FromStr for ReconciliationDecision {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "adopt" => Ok(Self::Adopt),
            "reapply" => Ok(Self::Reapply),
            "ask_user" => Ok(Self::AskUser),
            "abandon" => Ok(Self::Abandon),
            other => Err(format!("unknown reconciliation decision: {other}")),
        }
    }
}
