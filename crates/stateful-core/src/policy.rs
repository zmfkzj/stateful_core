use crate::Decision;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "path", rename_all = "snake_case")]
pub enum IntentScope {
    File(String),
    Directory(String),
}

impl IntentScope {
    pub fn file(path: impl AsRef<str>) -> Self {
        Self::File(normalize_relative_path(path.as_ref()))
    }

    pub fn directory(path: impl AsRef<str>) -> Self {
        Self::Directory(normalize_directory_path(path.as_ref()))
    }

    pub fn allows_write(&self, target: impl AsRef<str>) -> bool {
        let target = normalize_relative_path(target.as_ref());
        match self {
            Self::File(path) => path == &target,
            Self::Directory(scope) => {
                directory_depth(scope, &target).is_some_and(|depth| (1..=2).contains(&depth))
            }
        }
    }

    pub fn allows_delete(&self, target: impl AsRef<str>) -> bool {
        let target = normalize_relative_path(target.as_ref());
        matches!(self, Self::File(path) if path == &target)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeSet {
    scopes: Vec<IntentScope>,
}

impl ScopeSet {
    pub fn new(scopes: Vec<IntentScope>) -> Self {
        Self { scopes }
    }

    pub fn allows_write(&self, target: impl AsRef<str>) -> bool {
        let target = target.as_ref();
        self.scopes.iter().any(|scope| scope.allows_write(target))
    }

    pub fn allows_delete(&self, target: impl AsRef<str>) -> bool {
        let target = target.as_ref();
        self.scopes.iter().any(|scope| scope.allows_delete(target))
    }

    pub fn allows_rename(&self, old_path: impl AsRef<str>, new_path: impl AsRef<str>) -> bool {
        self.allows_delete(old_path) && self.allows_delete(new_path)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentPhase {
    Exploring,
    Editing,
    Testing,
    Blocked,
    Done,
    Failed,
}

impl IntentPhase {
    fn is_write_authorizing(self) -> bool {
        matches!(self, Self::Exploring | Self::Editing | Self::Testing)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyState {
    scopes: Option<ScopeSet>,
    phase: IntentPhase,
    finalized: bool,
    expired: bool,
}

impl Default for PolicyState {
    fn default() -> Self {
        Self {
            scopes: None,
            phase: IntentPhase::Editing,
            finalized: false,
            expired: false,
        }
    }
}

impl PolicyState {
    pub fn with_active_file_intent(mut self, path: impl AsRef<str>) -> Self {
        self.scopes = Some(ScopeSet::new(vec![IntentScope::file(path)]));
        self
    }

    pub fn with_active_directory_intent(mut self, path: impl AsRef<str>) -> Self {
        self.scopes = Some(ScopeSet::new(vec![IntentScope::directory(path)]));
        self
    }

    pub fn with_active_intent_scopes(mut self, scopes: Vec<IntentScope>) -> Self {
        self.scopes = Some(ScopeSet::new(scopes));
        self
    }

    pub fn with_phase(mut self, phase: IntentPhase) -> Self {
        self.phase = phase;
        self
    }

    pub fn with_expired_intent(mut self) -> Self {
        self.expired = true;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorizationInput {
    WriteFile { path: String },
    DeleteFile { path: String },
    RenameFile { old_path: String, new_path: String },
    MoveFile { old_path: String, new_path: String },
}

impl AuthorizationInput {
    pub fn write_file(path: impl AsRef<str>) -> Self {
        Self::WriteFile {
            path: normalize_relative_path(path.as_ref()),
        }
    }

    pub fn delete_file(path: impl AsRef<str>) -> Self {
        Self::DeleteFile {
            path: normalize_relative_path(path.as_ref()),
        }
    }

    pub fn rename_file(old_path: impl AsRef<str>, new_path: impl AsRef<str>) -> Self {
        Self::RenameFile {
            old_path: normalize_relative_path(old_path.as_ref()),
            new_path: normalize_relative_path(new_path.as_ref()),
        }
    }

    pub fn move_file(old_path: impl AsRef<str>, new_path: impl AsRef<str>) -> Self {
        Self::MoveFile {
            old_path: normalize_relative_path(old_path.as_ref()),
            new_path: normalize_relative_path(new_path.as_ref()),
        }
    }
}

pub fn authorize_action(state: &PolicyState, input: AuthorizationInput) -> Decision {
    if state.finalized {
        return Decision::deny(
            "finalized_session",
            "Session is finalized and cannot authorize writes.",
            "Declare a new intent in an active session before writing.",
        );
    }

    if !state.phase.is_write_authorizing() {
        return Decision::deny(
            match state.phase {
                IntentPhase::Blocked => "blocked_phase",
                IntentPhase::Done | IntentPhase::Failed => "finalized_session",
                IntentPhase::Exploring | IntentPhase::Editing | IntentPhase::Testing => {
                    "invalid_phase"
                }
            },
            "Current phase is not write-authorizing.",
            "Move the session back to exploring, editing, or testing with active intent.",
        );
    }

    if state.expired {
        return Decision::deny(
            "expired_intent",
            "Active intent has expired.",
            "Refresh or redeclare intent before writing.",
        );
    }

    let Some(scopes) = &state.scopes else {
        return Decision::deny(
            "missing_intent",
            "Supported writes require active file or directory intent.",
            "Call state.intent.declare with file or directory scope before writing.",
        );
    };

    match input {
        AuthorizationInput::WriteFile { path } if scopes.allows_write(&path) => {
            Decision::allow("authorized", "Write target is inside active intent scope.")
        }
        AuthorizationInput::DeleteFile { path } if scopes.allows_delete(&path) => {
            Decision::allow("authorized", "Delete target has exact active file intent.")
        }
        AuthorizationInput::RenameFile { old_path, new_path }
        | AuthorizationInput::MoveFile { old_path, new_path }
            if scopes.allows_rename(&old_path, &new_path) =>
        {
            Decision::allow(
                "authorized",
                "Rename or move source and destination have exact active file intent.",
            )
        }
        AuthorizationInput::WriteFile { .. }
        | AuthorizationInput::DeleteFile { .. }
        | AuthorizationInput::RenameFile { .. }
        | AuthorizationInput::MoveFile { .. } => Decision::deny(
            "scope_mismatch",
            "Target is outside active intent scope.",
            "Declare intent for the exact file, or for writes only a directory scope that covers the target.",
        ),
    }
}

fn normalize_directory_path(path: &str) -> String {
    let normalized = normalize_relative_path(path);
    if normalized.is_empty() {
        normalized
    } else {
        format!("{normalized}/")
    }
}

fn normalize_relative_path(path: &str) -> String {
    path.replace('\\', "/")
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .fold(Vec::new(), |mut segments, segment| {
            if segment == ".." {
                segments.pop();
            } else {
                segments.push(segment);
            }
            segments
        })
        .join("/")
}

fn directory_depth(scope: &str, target: &str) -> Option<usize> {
    let remainder = target.strip_prefix(scope)?;
    if remainder.is_empty() {
        return None;
    }

    Some(
        remainder
            .split('/')
            .filter(|segment| !segment.is_empty())
            .count(),
    )
}
