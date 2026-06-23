use crate::{Decision, normalize_directory_path, normalize_relative_path};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityPhase {
    Exploring,
    Editing,
    Testing,
    Blocked,
    Done,
    Failed,
}

impl ActivityPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exploring => "exploring",
            Self::Editing => "editing",
            Self::Testing => "testing",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    pub fn authorizes_writes(self) -> bool {
        matches!(self, Self::Exploring | Self::Editing | Self::Testing)
    }
}
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
        matches!(self, Self::File(path) if path == &target)
    }

    pub fn allows_write_directory(&self, target: impl AsRef<str>) -> bool {
        let target = normalize_directory_path(target.as_ref());
        matches!(self, Self::Directory(scope) if scope == &target)
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

    pub fn allows_write_directory(&self, target: impl AsRef<str>) -> bool {
        let target = target.as_ref();
        self.scopes
            .iter()
            .any(|scope| scope.allows_write_directory(target))
    }

    pub fn allows_delete(&self, target: impl AsRef<str>) -> bool {
        let target = target.as_ref();
        self.scopes.iter().any(|scope| scope.allows_delete(target))
    }

    pub fn allows_rename(&self, old_path: impl AsRef<str>, new_path: impl AsRef<str>) -> bool {
        self.allows_delete(old_path) && self.allows_delete(new_path)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyState {
    scopes: Option<ScopeSet>,
    phase: Option<ActivityPhase>,
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

    pub fn with_activity_phase(mut self, phase: ActivityPhase) -> Self {
        self.phase = Some(phase);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorizationInput {
    WriteFile { path: String },
    WriteDirectory { path: String },
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

    pub fn write_directory(path: impl AsRef<str>) -> Self {
        Self::WriteDirectory {
            path: normalize_directory_path(path.as_ref()),
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
    let Some(scopes) = &state.scopes else {
        return Decision::deny(
            "missing_intent",
            "Supported writes require active file or directory intent.",
            "Call state.intent.declare with file or directory scope before writing.",
        );
    };

    if let Some(phase) = state.phase
        && !phase.authorizes_writes()
    {
        return Decision::deny(
            "inactive_session_phase",
            "Session phase does not authorize writes.",
            "Move the session back to exploring, editing, or testing before writing.",
        );
    }

    match input {
        AuthorizationInput::WriteFile { path } if scopes.allows_write(&path) => {
            Decision::allow("authorized", "Write target has exact active file intent.")
        }
        AuthorizationInput::WriteDirectory { path } if scopes.allows_write_directory(&path) => {
            Decision::allow(
                "authorized",
                "Write directory target matches active directory intent.",
            )
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
        | AuthorizationInput::WriteDirectory { .. }
        | AuthorizationInput::DeleteFile { .. }
        | AuthorizationInput::RenameFile { .. }
        | AuthorizationInput::MoveFile { .. } => Decision::deny(
            "scope_mismatch",
            "Target is outside active intent scope.",
            "Declare exact file intent for file actions, or exact directory intent for write-directory actions.",
        ),
    }
}
