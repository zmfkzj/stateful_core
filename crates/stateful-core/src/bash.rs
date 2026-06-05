#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashKind {
    ReadOnly,
    Mutating,
    ValidationBypass,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BashClassification {
    pub kind: BashKind,
    pub reason: String,
}

pub fn classify_bash(_command: &str) -> BashClassification {
    BashClassification {
        kind: BashKind::Mutating,
        reason: "Bash commands require structured read-only sandbox metadata with network disabled"
            .to_string(),
    }
}
