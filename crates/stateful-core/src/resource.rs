use std::{
    fs::{self, File, Metadata},
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

use crate::{
    ContentDigest, DigestAlgorithm, DirectoryTreeState, EntryState, MutationOperation, ObjectKind,
    ObjectState, ResourceKey, ResourceKind, ResourceObservation,
};
use serde_json::Value;
use thiserror::Error;

#[cfg(unix)]
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};

#[derive(Debug, Error)]
pub enum ResourceError {
    #[error("workspace id is empty")]
    EmptyWorkspace,
    #[error("workspace root is not a directory: {0}")]
    InvalidWorkspace(String),
    #[error("path must stay inside the workspace: {0}")]
    InvalidRelativePath(String),
    #[error("path resolves outside the workspace: {0}")]
    WorkspaceEscape(String),
    #[error("expected a regular file: {0}")]
    NotRegularFile(String),
    #[error("expected a directory: {0}")]
    NotDirectory(String),
    #[error("expected an absent path: {0}")]
    NotAbsent(String),
    #[error("symlink files require an explicit symlink-entry operation: {0}")]
    SymlinkFile(String),
    #[error("resource changed while it was being observed: {0}")]
    UnstableObservation(String),
    #[error("directory tree contains a symlink: {0}")]
    SymlinkInDirectoryTree(String),
    #[error("directory tree contains a hardlinked regular file: {0}")]
    HardlinkedRegularFile(String),
    #[error("directory tree contains an unsupported filesystem object: {0}")]
    UnsupportedTreeEntry(String),
    #[error("operation requires an adapter-supplied observation: {0}")]
    AdapterSuppliedObservation(String),
    #[error("operation observation is invalid: {0}")]
    InvalidOperationObservation(String),
    #[error("operation transition is invalid: {0}")]
    InvalidOperationTransition(String),
    #[error("path is not valid UTF-8: {0:?}")]
    NonUtf8Path(PathBuf),
    #[error("filesystem operation failed for {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("filesystem identity is unsupported on this platform")]
    UnsupportedPlatform,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceSet {
    resources: Vec<ResourceKey>,
}

impl ResourceSet {
    pub fn new(mut resources: Vec<ResourceKey>) -> Self {
        resources.sort_by(|left, right| {
            (
                &left.workspace_id,
                left.kind,
                &left.resource_id,
                &left.canonical_path,
            )
                .cmp(&(
                    &right.workspace_id,
                    right.kind,
                    &right.resource_id,
                    &right.canonical_path,
                ))
        });
        resources.dedup_by(|left, right| {
            left.workspace_id == right.workspace_id
                && left.kind == right.kind
                && left.resource_id == right.resource_id
        });
        Self { resources }
    }

    pub fn resources(&self) -> &[ResourceKey] {
        &self.resources
    }

    pub fn into_resources(self) -> Vec<ResourceKey> {
        self.resources
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        // ponytail: resource batches are intentionally small; index resource ids if this becomes hot.
        self.resources.iter().any(|left| {
            other
                .resources
                .iter()
                .any(|right| resource_keys_overlap(left, right))
        })
    }
}

pub struct ResourceResolver {
    workspace_id: String,
    root: PathBuf,
}

impl ResourceResolver {
    pub fn new(
        workspace_id: impl Into<String>,
        root: impl AsRef<Path>,
    ) -> Result<Self, ResourceError> {
        let workspace_id = workspace_id.into();
        if workspace_id.trim().is_empty() {
            return Err(ResourceError::EmptyWorkspace);
        }
        let root_input = root.as_ref();
        let root = fs::canonicalize(root_input).map_err(|source| io_error(root_input, source))?;
        if !root.is_dir() {
            return Err(ResourceError::InvalidWorkspace(path_string(&root)?));
        }
        Ok(Self { workspace_id, root })
    }

    pub fn observe_existing_file(
        &self,
        relative_path: &str,
    ) -> Result<Vec<ResourceObservation>, ResourceError> {
        self.reject_symlink_components(relative_path, false, false)?;
        let lexical = self.join_validated(relative_path)?;
        let link_metadata =
            fs::symlink_metadata(&lexical).map_err(|source| io_error(&lexical, source))?;
        if link_metadata.file_type().is_symlink() {
            return Err(ResourceError::SymlinkFile(relative_path.to_string()));
        }
        let mut file = File::open(&lexical).map_err(|source| io_error(&lexical, source))?;
        let before = file
            .metadata()
            .map_err(|source| io_error(&lexical, source))?;
        if !before.is_file() {
            return Err(ResourceError::NotRegularFile(relative_path.to_string()));
        }
        let canonical = self.canonical_inside(&lexical)?;
        let digest = digest_reader(&mut file).map_err(|source| io_error(&lexical, source))?;
        let after = file
            .metadata()
            .map_err(|source| io_error(&lexical, source))?;
        let current = fs::metadata(&lexical).map_err(|source| io_error(&lexical, source))?;
        if !same_file_snapshot(&before, &after)?
            || !same_file_snapshot(&after, &current)?
            || self.canonical_inside(&lexical)? != canonical
        {
            return Err(ResourceError::UnstableObservation(
                relative_path.to_string(),
            ));
        }
        let object = self.object_key(&canonical, &after)?;
        let entry = self.entry_key_from_lexical(&lexical)?;
        let (device, inode) = metadata_identity(&after)?;
        Ok(vec![
            ResourceObservation::Object {
                resource: object,
                observed: ObjectState::Present {
                    kind: ObjectKind::RegularFile,
                    blake3: digest,
                    byte_len: after.len(),
                },
                generation: 0,
            },
            ResourceObservation::Entry {
                resource: entry,
                observed: EntryState::Present {
                    kind: ObjectKind::RegularFile,
                    device,
                    inode,
                    empty: None,
                },
                generation: 0,
            },
        ])
    }
    fn observe_existing_target(
        &self,
        relative_path: &str,
        entry_only: bool,
    ) -> Result<Vec<ResourceObservation>, ResourceError> {
        if entry_only {
            self.observe_existing_symlink_entry(relative_path)
        } else {
            self.observe_existing_file(relative_path)
        }
    }

    fn observe_existing_symlink_entry(
        &self,
        relative_path: &str,
    ) -> Result<Vec<ResourceObservation>, ResourceError> {
        self.reject_symlink_components(relative_path, false, true)?;
        let lexical = self.join_validated(relative_path)?;
        let before = fs::symlink_metadata(&lexical).map_err(|source| io_error(&lexical, source))?;
        if !before.file_type().is_symlink() {
            return Err(ResourceError::SymlinkFile(relative_path.to_string()));
        }
        let target = fs::read_link(&lexical).map_err(|source| io_error(&lexical, source))?;
        let (digest, byte_len) = symlink_target_digest(&target)?;
        let after = fs::symlink_metadata(&lexical).map_err(|source| io_error(&lexical, source))?;
        let current_target =
            fs::read_link(&lexical).map_err(|source| io_error(&lexical, source))?;
        if !after.file_type().is_symlink()
            || !same_file_snapshot(&before, &after)?
            || target != current_target
        {
            return Err(ResourceError::UnstableObservation(
                relative_path.to_string(),
            ));
        }
        let (device, inode) = metadata_identity(&after)?;
        Ok(vec![
            ResourceObservation::Object {
                resource: self.object_key(&lexical, &after)?,
                observed: ObjectState::Present {
                    kind: ObjectKind::Symlink,
                    blake3: digest,
                    byte_len,
                },
                generation: 0,
            },
            ResourceObservation::Entry {
                resource: self.entry_key_from_lexical(&lexical)?,
                observed: EntryState::Present {
                    kind: ObjectKind::Symlink,
                    device,
                    inode,
                    empty: None,
                },
                generation: 0,
            },
        ])
    }

    pub fn observe_operation(
        &self,
        operation: &MutationOperation,
    ) -> Result<Vec<ResourceObservation>, ResourceError> {
        let observations = match operation {
            MutationOperation::Create { path } | MutationOperation::Mkdir { path } => {
                self.observe_absent_entry(path)?
            }
            MutationOperation::Update { path } => self.observe_existing_file(path)?,
            MutationOperation::Delete { path, entry_only } => {
                self.observe_existing_target(path, *entry_only)?
            }
            MutationOperation::Rename {
                old_path,
                new_path,
                entry_only,
            }
            | MutationOperation::Move {
                old_path,
                new_path,
                entry_only,
            } => {
                let mut source = self.observe_existing_target(old_path, *entry_only)?;
                let destination = self.observe_absent_entry(new_path)?;
                if source != self.observe_existing_target(old_path, *entry_only)? {
                    return Err(ResourceError::UnstableObservation(old_path.clone()));
                }
                source.extend(destination);
                source
            }
            MutationOperation::Hardlink { old_path, new_path } => {
                let mut source = self.observe_existing_file(old_path)?;
                let destination = self.observe_absent_entry(new_path)?;
                if source != self.observe_existing_file(old_path)? {
                    return Err(ResourceError::UnstableObservation(old_path.clone()));
                }
                source.extend(destination);
                source
            }
            MutationOperation::Rmdir { path } => self.observe_rmdir(path)?,
            MutationOperation::WriteDirectory { path } => self.observe_directory_tree(path)?,
            MutationOperation::StructuredCommit { .. } => {
                return Err(ResourceError::AdapterSuppliedObservation(
                    "structured_commit .git metadata".to_string(),
                ));
            }
        };
        validate_operation_start(operation, &observations)?;
        Ok(observations)
    }

    fn observe_absent_entry(
        &self,
        relative_path: &str,
    ) -> Result<Vec<ResourceObservation>, ResourceError> {
        self.reject_symlink_components(relative_path, true, false)?;
        let lexical = self.join_validated(relative_path)?;
        match fs::symlink_metadata(&lexical) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(io_error(&lexical, source)),
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ResourceError::SymlinkFile(relative_path.to_string()));
            }
            Ok(_) => return Err(ResourceError::NotAbsent(relative_path.to_string())),
        }
        let entry = self.entry_key_from_lexical(&lexical)?;
        if !matches!(
            fs::symlink_metadata(&lexical),
            Err(error) if error.kind() == io::ErrorKind::NotFound
        ) || entry != self.entry_key_from_lexical(&lexical)?
        {
            return Err(ResourceError::UnstableObservation(
                relative_path.to_string(),
            ));
        }
        Ok(vec![ResourceObservation::Entry {
            resource: entry,
            observed: EntryState::Absent,
            generation: 0,
        }])
    }

    fn observe_rmdir(
        &self,
        relative_path: &str,
    ) -> Result<Vec<ResourceObservation>, ResourceError> {
        let tree = self.observe_directory_tree(relative_path)?;
        let ResourceObservation::DirectoryTree {
            observed:
                DirectoryTreeState::Present {
                    device,
                    inode,
                    entry_count,
                    ..
                },
            ..
        } = &tree[0]
        else {
            return Err(ResourceError::InvalidOperationObservation(
                "directory tree observation is absent".to_string(),
            ));
        };
        let lexical = self.join_validated(relative_path)?;
        let metadata =
            fs::symlink_metadata(&lexical).map_err(|source| io_error(&lexical, source))?;
        if metadata.file_type().is_symlink() {
            return Err(ResourceError::SymlinkFile(relative_path.to_string()));
        }
        let (current_device, current_inode) = metadata_identity(&metadata)?;
        if !metadata.is_dir() || (*device, *inode) != (current_device, current_inode) {
            return Err(ResourceError::UnstableObservation(
                relative_path.to_string(),
            ));
        }
        let entry = ResourceObservation::Entry {
            resource: self.entry_key_from_lexical(&lexical)?,
            observed: EntryState::Present {
                kind: ObjectKind::Directory,
                device: current_device,
                inode: current_inode,
                empty: Some(*entry_count == 0),
            },
            generation: 0,
        };
        if tree != self.observe_directory_tree(relative_path)? {
            return Err(ResourceError::UnstableObservation(
                relative_path.to_string(),
            ));
        }
        Ok(vec![tree[0].clone(), entry])
    }

    pub fn observe_directory_tree(
        &self,
        relative_path: &str,
    ) -> Result<Vec<ResourceObservation>, ResourceError> {
        self.reject_symlink_components(relative_path, false, false)?;
        let lexical = self.join_validated(relative_path)?;
        let metadata =
            fs::symlink_metadata(&lexical).map_err(|source| io_error(&lexical, source))?;
        if metadata.file_type().is_symlink() {
            return Err(ResourceError::SymlinkFile(relative_path.to_string()));
        }
        if !metadata.is_dir() {
            return Err(ResourceError::NotDirectory(relative_path.to_string()));
        }
        let canonical = self.canonical_inside(&lexical)?;
        let (before_snapshot, before_count) = self.directory_snapshot(&canonical)?;
        let after = fs::symlink_metadata(&lexical).map_err(|source| io_error(&lexical, source))?;
        let (after_snapshot, after_count) = self.directory_snapshot(&canonical)?;
        if !same_file_snapshot(&metadata, &after)?
            || self.canonical_inside(&lexical)? != canonical
            || before_snapshot != after_snapshot
            || before_count != after_count
        {
            return Err(ResourceError::UnstableObservation(
                relative_path.to_string(),
            ));
        }
        let (device, inode) = metadata_identity(&after)?;
        Ok(vec![ResourceObservation::DirectoryTree {
            resource: self.directory_key(&canonical, &after)?,
            observed: DirectoryTreeState::Present {
                device,
                inode,
                snapshot: after_snapshot,
                entry_count: after_count,
            },
            generation: 0,
        }])
    }

    #[cfg(test)]
    fn existing_file(&self, relative_path: &str) -> Result<ResourceSet, ResourceError> {
        Ok(ResourceSet::new(
            self.observe_existing_file(relative_path)?
                .into_iter()
                .map(|observation| observation.resource().clone())
                .collect(),
        ))
    }

    pub fn absent_entry(&self, relative_path: &str) -> Result<ResourceSet, ResourceError> {
        Ok(ResourceSet::new(
            self.observe_absent_entry(relative_path)?
                .into_iter()
                .map(|observation| observation.resource().clone())
                .collect(),
        ))
    }

    pub fn directory_tree(&self, relative_path: &str) -> Result<ResourceSet, ResourceError> {
        Ok(ResourceSet::new(
            self.observe_directory_tree(relative_path)?
                .into_iter()
                .map(|observation| observation.resource().clone())
                .collect(),
        ))
    }

    fn join_validated(&self, relative_path: &str) -> Result<PathBuf, ResourceError> {
        let normalized = validated_relative_path(relative_path)?;
        Ok(self.root.join(normalized))
    }
    fn reject_symlink_components(
        &self,
        relative_path: &str,
        allow_absent_final: bool,
        allow_final_symlink: bool,
    ) -> Result<(), ResourceError> {
        let normalized = validated_relative_path(relative_path)?;
        let target = self.root.join(&normalized);
        let mut current = self.root.clone();
        for component in normalized.components() {
            current.push(component.as_os_str());
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    if allow_final_symlink && current == target {
                        return Ok(());
                    }
                    return Err(ResourceError::SymlinkFile(relative_path.to_string()));
                }
                Ok(_) => {}
                Err(error)
                    if allow_absent_final
                        && error.kind() == io::ErrorKind::NotFound
                        && current == target =>
                {
                    return Ok(());
                }
                Err(source) => return Err(io_error(&current, source)),
            }
        }
        Ok(())
    }

    fn canonical_inside(&self, path: &Path) -> Result<PathBuf, ResourceError> {
        let canonical = fs::canonicalize(path).map_err(|source| io_error(path, source))?;
        if !canonical.starts_with(&self.root) {
            return Err(ResourceError::WorkspaceEscape(path_string(path)?));
        }
        Ok(canonical)
    }

    fn object_key(&self, path: &Path, metadata: &Metadata) -> Result<ResourceKey, ResourceError> {
        Ok(ResourceKey {
            workspace_id: self.workspace_id.clone(),
            kind: ResourceKind::Object,
            resource_id: format!("object:{}", filesystem_identity(metadata)?),
            canonical_path: self.relative_string(path)?,
        })
    }

    fn directory_key(
        &self,
        path: &Path,
        metadata: &Metadata,
    ) -> Result<ResourceKey, ResourceError> {
        Ok(ResourceKey {
            workspace_id: self.workspace_id.clone(),
            kind: ResourceKind::DirectoryTree,
            resource_id: format!("directory:{}", filesystem_identity(metadata)?),
            canonical_path: self.relative_string(path)?,
        })
    }

    fn entry_key_from_lexical(&self, lexical: &Path) -> Result<ResourceKey, ResourceError> {
        let Some(name) = lexical.file_name().and_then(|name| name.to_str()) else {
            return Err(ResourceError::InvalidRelativePath(path_string(lexical)?));
        };
        let parent = lexical.parent().unwrap_or(&self.root);
        let canonical_parent = self.canonical_inside(parent)?;
        let parent_relative = self.relative_string(&canonical_parent)?;
        let canonical_path = if parent_relative.is_empty() {
            name.to_string()
        } else {
            format!("{parent_relative}/{name}")
        };
        let parent_metadata = fs::metadata(&canonical_parent)
            .map_err(|source| io_error(&canonical_parent, source))?;
        Ok(ResourceKey {
            workspace_id: self.workspace_id.clone(),
            kind: ResourceKind::Entry,
            resource_id: format!("entry:{}:{name}", filesystem_identity(&parent_metadata)?),
            canonical_path,
        })
    }

    fn relative_string(&self, path: &Path) -> Result<String, ResourceError> {
        let relative = match path.strip_prefix(&self.root) {
            Ok(relative) => relative,
            Err(_) => return Err(ResourceError::WorkspaceEscape(path_string(path)?)),
        };
        path_string(relative)
    }
    fn directory_snapshot(&self, directory: &Path) -> Result<(ContentDigest, u64), ResourceError> {
        let mut hasher = blake3::Hasher::new();
        let mut entry_count = 0;
        self.hash_tree_entry(directory, &mut hasher, &mut entry_count)?;
        Ok((
            ContentDigest {
                algorithm: DigestAlgorithm::Blake3,
                value: hasher.finalize().to_hex().to_string(),
            },
            entry_count,
        ))
    }

    fn hash_tree_entry(
        &self,
        path: &Path,
        hasher: &mut blake3::Hasher,
        entry_count: &mut u64,
    ) -> Result<(), ResourceError> {
        let metadata = fs::symlink_metadata(path).map_err(|source| io_error(path, source))?;
        let relative = self.relative_string(path)?;
        if metadata.file_type().is_symlink() {
            return Err(ResourceError::SymlinkInDirectoryTree(relative));
        }
        if metadata.is_dir() {
            hash_tree_metadata(hasher, b"d", &relative, &metadata)?;
            let mut entries = Vec::new();
            for entry in fs::read_dir(path).map_err(|source| io_error(path, source))? {
                let entry = entry.map_err(|source| io_error(path, source))?;
                let entry_path = entry.path();
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| ResourceError::NonUtf8Path(entry_path.clone()))?;
                entries.push((name, entry_path));
            }
            entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
            for (_, child) in entries {
                *entry_count += 1;
                self.hash_tree_entry(&child, hasher, entry_count)?;
            }
            return Ok(());
        }
        if !metadata.is_file() {
            return Err(ResourceError::UnsupportedTreeEntry(relative));
        }
        if regular_file_has_multiple_links(&metadata)? {
            return Err(ResourceError::HardlinkedRegularFile(relative));
        }
        let mut file = File::open(path).map_err(|source| io_error(path, source))?;
        let before = file.metadata().map_err(|source| io_error(path, source))?;
        let digest = digest_reader(&mut file).map_err(|source| io_error(path, source))?;
        let after = file.metadata().map_err(|source| io_error(path, source))?;
        let current = fs::symlink_metadata(path).map_err(|source| io_error(path, source))?;
        if current.file_type().is_symlink()
            || !same_file_snapshot(&before, &after)?
            || !same_file_snapshot(&after, &current)?
        {
            return Err(ResourceError::UnstableObservation(relative));
        }
        hash_tree_metadata(hasher, b"f", &self.relative_string(path)?, &after)?;
        hash_tree_field(hasher, b"digest", digest.value.as_bytes());
        Ok(())
    }
}
pub fn validate_operation_start(
    operation: &MutationOperation,
    observations: &[ResourceObservation],
) -> Result<(), ResourceError> {
    match operation {
        MutationOperation::Create { path } | MutationOperation::Mkdir { path } => {
            validate_absent_entry(observations, &operation_path(path)?)?;
        }
        MutationOperation::Update { path } => {
            validate_regular_file(observations, &operation_path(path)?)?;
        }
        MutationOperation::Delete { path, entry_only } => {
            validate_existing_target(observations, &operation_path(path)?, *entry_only)?;
        }
        MutationOperation::Rename {
            old_path,
            new_path,
            entry_only,
        }
        | MutationOperation::Move {
            old_path,
            new_path,
            entry_only,
        } => {
            let old_path = operation_path(old_path)?;
            let new_path = operation_path(new_path)?;
            if old_path == new_path {
                return Err(ResourceError::InvalidOperationObservation(
                    "source and destination paths must differ".to_string(),
                ));
            }
            require_shape(
                observations,
                &[
                    (ResourceKind::Object, old_path.clone()),
                    (ResourceKind::Entry, old_path.clone()),
                    (ResourceKind::Entry, new_path.clone()),
                ],
            )?;
            validate_existing_target_from_set(observations, &old_path, *entry_only)?;
            validate_absent_entry_from_set(observations, &new_path)?;
        }
        MutationOperation::Hardlink { old_path, new_path } => {
            let old_path = operation_path(old_path)?;
            let new_path = operation_path(new_path)?;
            if old_path == new_path {
                return Err(ResourceError::InvalidOperationObservation(
                    "source and destination paths must differ".to_string(),
                ));
            }
            require_shape(
                observations,
                &[
                    (ResourceKind::Object, old_path.clone()),
                    (ResourceKind::Entry, old_path.clone()),
                    (ResourceKind::Entry, new_path.clone()),
                ],
            )?;
            validate_regular_file_from_set(observations, &old_path)?;
            validate_absent_entry_from_set(observations, &new_path)?;
        }
        MutationOperation::Rmdir { path } => {
            validate_directory(observations, &operation_path(path)?, true)?;
        }
        MutationOperation::WriteDirectory { path } => {
            let path = operation_path(path)?;
            require_shape(observations, &[(ResourceKind::DirectoryTree, path.clone())])?;
            validate_directory_tree_from_set(observations, &path)?;
        }
        MutationOperation::StructuredCommit { tracked_paths } => {
            validate_structured_commit_start(tracked_paths, observations)?;
        }
    }
    Ok(())
}

pub fn validate_operation_transition(
    operation: &MutationOperation,
    start: &[ResourceObservation],
    expected: &[ResourceObservation],
    actual: &[ResourceObservation],
) -> Result<(), ResourceError> {
    validate_operation_start(operation, start)?;
    validate_expected_transition(operation, start, expected)?;
    if !observations_equal_as_set(expected, actual) {
        return Err(ResourceError::InvalidOperationTransition(
            "actual post-state does not exactly match expected post-state".to_string(),
        ));
    }
    Ok(())
}

fn validate_expected_transition(
    operation: &MutationOperation,
    start: &[ResourceObservation],
    expected: &[ResourceObservation],
) -> Result<(), ResourceError> {
    match operation {
        MutationOperation::Create { path } => {
            let path = operation_path(path)?;
            let start_entry = validate_absent_entry(start, &path)?;
            let (_, entry) = validate_regular_file(expected, &path)?;
            require_same_resource(start_entry, entry, "create destination entry")?;
        }
        MutationOperation::Update { path } => {
            let path = operation_path(path)?;
            let (start_object, start_entry) = validate_regular_file(start, &path)?;
            let (post_object, post_entry) = validate_regular_file(expected, &path)?;
            require_same_resource(start_entry, post_entry, "update entry")?;
            if start_object.resource().resource_id == post_object.resource().resource_id
                && object_state(start_object)? == object_state(post_object)?
            {
                return Err(ResourceError::InvalidOperationTransition(
                    "update post-state must change content or rebind object identity".to_string(),
                ));
            }
        }
        MutationOperation::Delete { path, entry_only } => {
            let path = operation_path(path)?;
            let (_, start_entry) = validate_existing_target(start, &path, *entry_only)?;
            let post_entry = validate_absent_entry(expected, &path)?;
            require_same_resource(start_entry, post_entry, "delete source entry")?;
        }
        MutationOperation::Rename {
            old_path,
            new_path,
            entry_only,
        }
        | MutationOperation::Move {
            old_path,
            new_path,
            entry_only,
        } => {
            let old_path = operation_path(old_path)?;
            let new_path = operation_path(new_path)?;
            let (start_object, start_entry) =
                validate_existing_target_from_set(start, &old_path, *entry_only)?;
            let destination_start = validate_absent_entry_from_set(start, &new_path)?;
            require_shape(
                expected,
                &[
                    (ResourceKind::Object, new_path.clone()),
                    (ResourceKind::Entry, old_path.clone()),
                    (ResourceKind::Entry, new_path.clone()),
                ],
            )?;
            let (post_object, destination_post) =
                validate_existing_target_from_set(expected, &new_path, *entry_only)?;
            let source_post = validate_absent_entry_from_set(expected, &old_path)?;
            if start_object.resource().resource_id != post_object.resource().resource_id {
                return Err(ResourceError::InvalidOperationTransition(
                    "rename/move must preserve source object identity".to_string(),
                ));
            }
            require_same_resource(start_entry, source_post, "rename/move source entry")?;
            require_same_resource(
                destination_start,
                destination_post,
                "rename/move destination entry",
            )?;
        }
        MutationOperation::Hardlink { old_path, new_path } => {
            let old_path = operation_path(old_path)?;
            let new_path = operation_path(new_path)?;
            let (start_object, start_entry) = validate_regular_file_from_set(start, &old_path)?;
            let destination_start = validate_absent_entry_from_set(start, &new_path)?;
            require_shape(
                expected,
                &[
                    (ResourceKind::Object, old_path.clone()),
                    (ResourceKind::Entry, old_path.clone()),
                    (ResourceKind::Entry, new_path.clone()),
                ],
            )?;
            let (post_object, source_post) = validate_regular_file_from_set(expected, &old_path)?;
            let destination_post = observation_at(expected, ResourceKind::Entry, &new_path)?;
            validate_present_regular_entry(destination_post, &new_path)?;
            require_same_resource(start_object, post_object, "hardlink source object")?;
            require_same_resource(start_entry, source_post, "hardlink source entry")?;
            require_same_resource(
                destination_start,
                destination_post,
                "hardlink destination entry",
            )?;
            require_entry_matches_object(destination_post, post_object)?;
        }
        MutationOperation::Mkdir { path } => {
            let path = operation_path(path)?;
            let start_entry = validate_absent_entry(start, &path)?;
            let (_, entry) = validate_directory(expected, &path, true)?;
            require_same_resource(start_entry, entry, "mkdir destination entry")?;
        }
        MutationOperation::Rmdir { path } => {
            let path = operation_path(path)?;
            let (_, start_entry) = validate_directory(start, &path, true)?;
            let post_entry = validate_absent_entry(expected, &path)?;
            require_same_resource(start_entry, post_entry, "rmdir source entry")?;
        }
        MutationOperation::WriteDirectory { path } => {
            let path = operation_path(path)?;
            let start_tree = validate_directory_tree(start, &path)?;
            let post_tree = validate_directory_tree(expected, &path)?;
            require_same_resource(start_tree, post_tree, "write-directory tree")?;
            require_directory_identity(start_tree, post_tree)?;
        }
        MutationOperation::StructuredCommit { tracked_paths } => {
            validate_structured_commit_start(tracked_paths, expected)?;
            validate_structured_commit_transition(start, expected)?;
        }
    }
    Ok(())
}

fn validate_structured_commit_start(
    tracked_paths: &[String],
    observations: &[ResourceObservation],
) -> Result<(), ResourceError> {
    let tracked_paths = normalized_unique_paths(tracked_paths)?;
    let mut shape = Vec::with_capacity(tracked_paths.len() * 2 + 1);
    let mut present_paths = Vec::new();
    for path in &tracked_paths {
        if path == ".git" {
            return Err(ResourceError::InvalidOperationObservation(
                "structured_commit tracked paths must not include .git".to_string(),
            ));
        }
        let entry = observation_at(observations, ResourceKind::Entry, path)?;
        shape.push((ResourceKind::Entry, path.clone()));
        match entry {
            ResourceObservation::Entry {
                observed: EntryState::Absent,
                ..
            } => {}
            ResourceObservation::Entry {
                observed:
                    EntryState::Present {
                        kind: ObjectKind::RegularFile,
                        ..
                    },
                ..
            } => {
                shape.push((ResourceKind::Object, path.clone()));
                present_paths.push(path.clone());
            }
            _ => return invalid_observation("structured_commit target is not a regular file"),
        }
    }
    shape.push((ResourceKind::DirectoryTree, ".git".to_string()));
    require_shape(observations, &shape)?;
    for path in &tracked_paths {
        if present_paths.contains(path) {
            validate_regular_file_from_set(observations, path)?;
        } else {
            validate_absent_entry_from_set(observations, path)?;
        }
    }
    validate_directory_tree_from_set(observations, ".git")?;
    Ok(())
}

fn validate_structured_commit_transition(
    start: &[ResourceObservation],
    expected: &[ResourceObservation],
) -> Result<(), ResourceError> {
    let start_git = validate_directory_tree_from_set(start, ".git")?;
    let expected_git = validate_directory_tree_from_set(expected, ".git")?;
    require_same_resource(start_git, expected_git, "structured-commit .git metadata")?;
    require_directory_identity(start_git, expected_git)?;
    let (
        DirectoryTreeState::Present {
            snapshot: start_snapshot,
            ..
        },
        DirectoryTreeState::Present {
            snapshot: expected_snapshot,
            ..
        },
    ) = (
        directory_tree_state(start_git)?,
        directory_tree_state(expected_git)?,
    )
    else {
        return Err(ResourceError::InvalidOperationTransition(
            "structured-commit .git metadata must be present".to_string(),
        ));
    };
    if start_snapshot == expected_snapshot {
        return Err(ResourceError::InvalidOperationTransition(
            "structured-commit .git metadata snapshot must change".to_string(),
        ));
    }
    for observation in start {
        if observation.resource() != expected_git.resource()
            && !expected
                .iter()
                .any(|candidate| observations_same_state(candidate, observation))
        {
            return Err(ResourceError::InvalidOperationTransition(
                "structured-commit tracked observations must remain unchanged".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_regular_file<'a>(
    observations: &'a [ResourceObservation],
    path: &str,
) -> Result<(&'a ResourceObservation, &'a ResourceObservation), ResourceError> {
    require_shape(
        observations,
        &[
            (ResourceKind::Object, path.to_string()),
            (ResourceKind::Entry, path.to_string()),
        ],
    )?;
    validate_regular_file_from_set(observations, path)
}

fn validate_regular_file_from_set<'a>(
    observations: &'a [ResourceObservation],
    path: &str,
) -> Result<(&'a ResourceObservation, &'a ResourceObservation), ResourceError> {
    let object = observation_at(observations, ResourceKind::Object, path)?;
    let entry = observation_at(observations, ResourceKind::Entry, path)?;
    let ResourceObservation::Object {
        resource,
        observed: ObjectState::Present { kind, blake3, .. },
        ..
    } = object
    else {
        return invalid_observation("regular object is missing");
    };
    if *kind != ObjectKind::RegularFile || blake3.algorithm != DigestAlgorithm::Blake3 {
        return invalid_observation("object is not a BLAKE3-observed regular file");
    }
    validate_present_regular_entry(entry, path)?;
    let ResourceObservation::Entry {
        observed: EntryState::Present { device, inode, .. },
        ..
    } = entry
    else {
        return invalid_observation("regular entry is missing");
    };
    if resource.resource_id != format!("object:{device}:{inode}") {
        return invalid_observation("object and entry identities differ");
    }
    Ok((object, entry))
}
fn validate_existing_target<'a>(
    observations: &'a [ResourceObservation],
    path: &str,
    entry_only: bool,
) -> Result<(&'a ResourceObservation, &'a ResourceObservation), ResourceError> {
    if entry_only {
        validate_symlink_entry(observations, path)
    } else {
        validate_regular_file(observations, path)
    }
}

fn validate_existing_target_from_set<'a>(
    observations: &'a [ResourceObservation],
    path: &str,
    entry_only: bool,
) -> Result<(&'a ResourceObservation, &'a ResourceObservation), ResourceError> {
    if entry_only {
        validate_symlink_entry_from_set(observations, path)
    } else {
        validate_regular_file_from_set(observations, path)
    }
}

fn validate_symlink_entry<'a>(
    observations: &'a [ResourceObservation],
    path: &str,
) -> Result<(&'a ResourceObservation, &'a ResourceObservation), ResourceError> {
    require_shape(
        observations,
        &[
            (ResourceKind::Object, path.to_string()),
            (ResourceKind::Entry, path.to_string()),
        ],
    )?;
    validate_symlink_entry_from_set(observations, path)
}

fn validate_symlink_entry_from_set<'a>(
    observations: &'a [ResourceObservation],
    path: &str,
) -> Result<(&'a ResourceObservation, &'a ResourceObservation), ResourceError> {
    let object = observation_at(observations, ResourceKind::Object, path)?;
    let entry = observation_at(observations, ResourceKind::Entry, path)?;
    let ResourceObservation::Object {
        resource,
        observed: ObjectState::Present { kind, blake3, .. },
        ..
    } = object
    else {
        return invalid_observation("symlink object is missing");
    };
    if *kind != ObjectKind::Symlink || blake3.algorithm != DigestAlgorithm::Blake3 {
        return invalid_observation("object is not a BLAKE3-observed symlink");
    }
    validate_present_symlink_entry(entry, path)?;
    let ResourceObservation::Entry {
        observed: EntryState::Present { device, inode, .. },
        ..
    } = entry
    else {
        return invalid_observation("symlink entry is missing");
    };
    if resource.resource_id != format!("object:{device}:{inode}") {
        return invalid_observation("symlink object and entry identities differ");
    }
    Ok((object, entry))
}

fn validate_absent_entry<'a>(
    observations: &'a [ResourceObservation],
    path: &str,
) -> Result<&'a ResourceObservation, ResourceError> {
    require_shape(observations, &[(ResourceKind::Entry, path.to_string())])?;
    validate_absent_entry_from_set(observations, path)
}

fn validate_absent_entry_from_set<'a>(
    observations: &'a [ResourceObservation],
    path: &str,
) -> Result<&'a ResourceObservation, ResourceError> {
    let entry = observation_at(observations, ResourceKind::Entry, path)?;
    let ResourceObservation::Entry {
        resource,
        observed: EntryState::Absent,
        ..
    } = entry
    else {
        return invalid_observation("destination entry must be absent");
    };
    validate_entry_resource(resource, path)?;
    Ok(entry)
}

fn validate_present_regular_entry(
    observation: &ResourceObservation,
    path: &str,
) -> Result<(), ResourceError> {
    let ResourceObservation::Entry {
        resource,
        observed: EntryState::Present { kind, empty, .. },
        ..
    } = observation
    else {
        return invalid_observation("regular entry is missing");
    };
    if *kind != ObjectKind::RegularFile || empty.is_some() {
        return invalid_observation("entry is not a regular file");
    }
    validate_entry_resource(resource, path)
}
fn validate_present_symlink_entry(
    observation: &ResourceObservation,
    path: &str,
) -> Result<(), ResourceError> {
    let ResourceObservation::Entry {
        resource,
        observed: EntryState::Present { kind, empty, .. },
        ..
    } = observation
    else {
        return invalid_observation("symlink entry is missing");
    };
    if *kind != ObjectKind::Symlink || empty.is_some() {
        return invalid_observation("entry is not a symlink");
    }
    validate_entry_resource(resource, path)
}

fn validate_directory<'a>(
    observations: &'a [ResourceObservation],
    path: &str,
    require_empty: bool,
) -> Result<(&'a ResourceObservation, &'a ResourceObservation), ResourceError> {
    require_shape(
        observations,
        &[
            (ResourceKind::DirectoryTree, path.to_string()),
            (ResourceKind::Entry, path.to_string()),
        ],
    )?;
    let tree = validate_directory_tree_from_set(observations, path)?;
    let entry = observation_at(observations, ResourceKind::Entry, path)?;
    let DirectoryTreeState::Present {
        device,
        inode,
        entry_count,
        ..
    } = directory_tree_state(tree)?
    else {
        return invalid_observation("directory tree is absent");
    };
    let ResourceObservation::Entry {
        resource,
        observed:
            EntryState::Present {
                kind,
                device: entry_device,
                inode: entry_inode,
                empty,
            },
        ..
    } = entry
    else {
        return invalid_observation("directory entry is missing");
    };
    if *kind != ObjectKind::Directory
        || (*device, *inode) != (*entry_device, *entry_inode)
        || *empty != Some(*entry_count == 0)
    {
        return invalid_observation("directory tree and entry differ");
    }
    if require_empty && *entry_count != 0 {
        return invalid_observation("rmdir/mkdir directory must be empty");
    }
    validate_entry_resource(resource, path)?;
    Ok((tree, entry))
}

fn validate_directory_tree<'a>(
    observations: &'a [ResourceObservation],
    path: &str,
) -> Result<&'a ResourceObservation, ResourceError> {
    require_shape(
        observations,
        &[(ResourceKind::DirectoryTree, path.to_string())],
    )?;
    validate_directory_tree_from_set(observations, path)
}

fn validate_directory_tree_from_set<'a>(
    observations: &'a [ResourceObservation],
    path: &str,
) -> Result<&'a ResourceObservation, ResourceError> {
    let tree = observation_at(observations, ResourceKind::DirectoryTree, path)?;
    let ResourceObservation::DirectoryTree { resource, .. } = tree else {
        return invalid_observation("directory tree is missing");
    };
    let DirectoryTreeState::Present {
        device,
        inode,
        snapshot,
        ..
    } = directory_tree_state(tree)?
    else {
        return invalid_observation("directory tree is absent");
    };
    if resource.resource_id != format!("directory:{device}:{inode}")
        || snapshot.algorithm != DigestAlgorithm::Blake3
    {
        return invalid_observation("directory tree identity or digest is invalid");
    }
    Ok(tree)
}

fn object_state(observation: &ResourceObservation) -> Result<&ObjectState, ResourceError> {
    let ResourceObservation::Object { observed, .. } = observation else {
        return invalid_observation("resource is not an object");
    };
    Ok(observed)
}

fn directory_tree_state(
    observation: &ResourceObservation,
) -> Result<&DirectoryTreeState, ResourceError> {
    let ResourceObservation::DirectoryTree { observed, .. } = observation else {
        return invalid_observation("resource is not a directory tree");
    };
    Ok(observed)
}

fn require_shape(
    observations: &[ResourceObservation],
    shape: &[(ResourceKind, String)],
) -> Result<(), ResourceError> {
    if observations.len() != shape.len() || observations.is_empty() {
        return invalid_observation("resource observation count is not exact");
    }
    let workspace_id = &observations[0].resource().workspace_id;
    if workspace_id.is_empty()
        || observations.iter().any(|observation| {
            !observation.has_matching_resource_kind()
                || observation.resource().workspace_id != *workspace_id
        })
    {
        return invalid_observation("resource observations have invalid workspace or kind");
    }
    for (kind, path) in shape {
        if observations
            .iter()
            .filter(|observation| {
                observation.resource().kind == *kind
                    && observation.resource().canonical_path == *path
            })
            .count()
            != 1
        {
            return invalid_observation("required resource key or canonical path is missing");
        }
    }
    for (index, observation) in observations.iter().enumerate() {
        if observations[..index]
            .iter()
            .any(|other| other.resource() == observation.resource())
        {
            return invalid_observation("duplicate resource observation");
        }
        if !shape.iter().any(|(kind, path)| {
            observation.resource().kind == *kind && observation.resource().canonical_path == *path
        }) {
            return invalid_observation("unexpected resource key or canonical path");
        }
    }
    Ok(())
}

fn observation_at<'a>(
    observations: &'a [ResourceObservation],
    kind: ResourceKind,
    path: &str,
) -> Result<&'a ResourceObservation, ResourceError> {
    let mut matches = observations.iter().filter(|observation| {
        observation.resource().kind == kind && observation.resource().canonical_path == path
    });
    let Some(observation) = matches.next() else {
        return invalid_observation("required resource observation is missing");
    };
    if matches.next().is_some() {
        return invalid_observation("resource observation is ambiguous");
    }
    Ok(observation)
}

fn validate_entry_resource(resource: &ResourceKey, path: &str) -> Result<(), ResourceError> {
    let Some(name) = path.rsplit('/').next() else {
        return invalid_observation("entry path has no basename");
    };
    if !resource.resource_id.starts_with("entry:")
        || !resource.resource_id.ends_with(&format!(":{name}"))
    {
        return invalid_observation("entry physical key does not match canonical path");
    }
    Ok(())
}

fn require_same_resource(
    left: &ResourceObservation,
    right: &ResourceObservation,
    role: &str,
) -> Result<(), ResourceError> {
    if left.resource() != right.resource() {
        return Err(ResourceError::InvalidOperationTransition(format!(
            "{role} resource key changed"
        )));
    }
    Ok(())
}
fn require_entry_matches_object(
    entry: &ResourceObservation,
    object: &ResourceObservation,
) -> Result<(), ResourceError> {
    let ResourceObservation::Entry {
        observed: EntryState::Present { device, inode, .. },
        ..
    } = entry
    else {
        return invalid_transition("hardlink destination entry is absent");
    };
    if object.resource().resource_id != format!("object:{device}:{inode}") {
        return invalid_transition("hardlink destination does not share source object identity");
    }
    Ok(())
}

fn require_directory_identity(
    left: &ResourceObservation,
    right: &ResourceObservation,
) -> Result<(), ResourceError> {
    let (
        DirectoryTreeState::Present {
            device: left_device,
            inode: left_inode,
            ..
        },
        DirectoryTreeState::Present {
            device: right_device,
            inode: right_inode,
            ..
        },
    ) = (directory_tree_state(left)?, directory_tree_state(right)?)
    else {
        return invalid_transition("directory tree must remain present");
    };
    if (left_device, left_inode) != (right_device, right_inode) {
        return invalid_transition("directory identity changed");
    }
    Ok(())
}

fn observations_equal_as_set(
    expected: &[ResourceObservation],
    actual: &[ResourceObservation],
) -> bool {
    expected.len() == actual.len()
        && expected.iter().all(|observation| {
            actual
                .iter()
                .any(|candidate| observations_same_state(candidate, observation))
        })
}

fn observations_same_state(left: &ResourceObservation, right: &ResourceObservation) -> bool {
    if left.resource() != right.resource() {
        return false;
    }
    match (left, right) {
        (
            ResourceObservation::Object { observed: left, .. },
            ResourceObservation::Object {
                observed: right, ..
            },
        ) => left == right,
        (
            ResourceObservation::Entry { observed: left, .. },
            ResourceObservation::Entry {
                observed: right, ..
            },
        ) => left == right,
        (
            ResourceObservation::DirectoryTree { observed: left, .. },
            ResourceObservation::DirectoryTree {
                observed: right, ..
            },
        ) => left == right,
        _ => false,
    }
}

fn normalized_unique_paths(paths: &[String]) -> Result<Vec<String>, ResourceError> {
    if paths.is_empty() {
        return invalid_observation("structured_commit requires tracked paths");
    }
    let mut normalized = Vec::with_capacity(paths.len());
    for path in paths {
        let path = operation_path(path)?;
        if normalized.contains(&path) {
            return invalid_observation("structured_commit tracked path is duplicated");
        }
        normalized.push(path);
    }
    Ok(normalized)
}

fn operation_path(path: &str) -> Result<String, ResourceError> {
    path_string(&validated_relative_path(path)?)
}

fn invalid_observation<T>(message: &str) -> Result<T, ResourceError> {
    Err(ResourceError::InvalidOperationObservation(
        message.to_string(),
    ))
}

fn invalid_transition<T>(message: &str) -> Result<T, ResourceError> {
    Err(ResourceError::InvalidOperationTransition(
        message.to_string(),
    ))
}

pub fn digest_bytes(bytes: &[u8]) -> ContentDigest {
    ContentDigest {
        algorithm: DigestAlgorithm::Blake3,
        value: blake3::hash(bytes).to_hex().to_string(),
    }
}

pub fn digest_canonical_json(value: &Value) -> ContentDigest {
    let mut hasher = blake3::Hasher::new();
    hash_json_value(value, &mut hasher);
    ContentDigest {
        algorithm: DigestAlgorithm::Blake3,
        value: hasher.finalize().to_hex().to_string(),
    }
}

fn hash_json_value(value: &Value, hasher: &mut blake3::Hasher) {
    match value {
        Value::Null => {
            hasher.update(b"n");
        }
        Value::Bool(value) => {
            hasher.update(if *value { b"b1" } else { b"b0" });
        }
        Value::Number(value) => hash_json_atom(b'd', value.to_string().as_bytes(), hasher),
        Value::String(value) => hash_json_atom(b's', value.as_bytes(), hasher),
        Value::Array(values) => {
            hasher.update(b"a");
            hasher.update(&(values.len() as u64).to_be_bytes());
            for value in values {
                hash_json_value(value, hasher);
            }
        }
        Value::Object(values) => {
            hasher.update(b"o");
            hasher.update(&(values.len() as u64).to_be_bytes());
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for key in keys {
                hash_json_atom(b'k', key.as_bytes(), hasher);
                hash_json_value(&values[key], hasher);
            }
        }
    }
}

fn hash_json_atom(kind: u8, bytes: &[u8], hasher: &mut blake3::Hasher) {
    hasher.update(&[kind]);
    hasher.update(&(bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

pub fn digest_reader(mut reader: impl Read) -> io::Result<ContentDigest> {
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(ContentDigest {
        algorithm: DigestAlgorithm::Blake3,
        value: hasher.finalize().to_hex().to_string(),
    })
}
#[cfg(unix)]
fn symlink_target_digest(target: &Path) -> Result<(ContentDigest, u64), ResourceError> {
    let bytes = target.as_os_str().as_bytes();
    Ok((digest_bytes(bytes), bytes.len() as u64))
}

#[cfg(not(unix))]
fn symlink_target_digest(_target: &Path) -> Result<(ContentDigest, u64), ResourceError> {
    Err(ResourceError::UnsupportedPlatform)
}

fn hash_tree_metadata(
    hasher: &mut blake3::Hasher,
    kind: &[u8],
    relative_path: &str,
    metadata: &Metadata,
) -> Result<(), ResourceError> {
    hash_tree_field(hasher, b"kind", kind);
    hash_tree_field(hasher, b"path", relative_path.as_bytes());
    hash_tree_field(
        hasher,
        b"identity",
        filesystem_identity(metadata)?.as_bytes(),
    );
    hash_tree_field(hasher, b"byte_len", &metadata.len().to_be_bytes());
    Ok(())
}

fn hash_tree_field(hasher: &mut blake3::Hasher, name: &[u8], value: &[u8]) {
    hasher.update(&(name.len() as u64).to_be_bytes());
    hasher.update(name);
    hasher.update(&(value.len() as u64).to_be_bytes());
    hasher.update(value);
}

#[cfg(unix)]
fn regular_file_has_multiple_links(metadata: &Metadata) -> Result<bool, ResourceError> {
    Ok(metadata.nlink() > 1)
}

#[cfg(not(unix))]
fn regular_file_has_multiple_links(_metadata: &Metadata) -> Result<bool, ResourceError> {
    Err(ResourceError::UnsupportedPlatform)
}

pub fn resource_keys_overlap(left: &ResourceKey, right: &ResourceKey) -> bool {
    if left.workspace_id != right.workspace_id {
        return false;
    }
    if left.kind == right.kind && left.resource_id == right.resource_id {
        return true;
    }
    if left.canonical_path == right.canonical_path {
        return true;
    }
    (left.kind == ResourceKind::DirectoryTree
        && path_contains(&left.canonical_path, &right.canonical_path))
        || (right.kind == ResourceKind::DirectoryTree
            && path_contains(&right.canonical_path, &left.canonical_path))
}

fn path_contains(directory: &str, path: &str) -> bool {
    directory.is_empty()
        || path
            .strip_prefix(directory)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn validated_relative_path(path: &str) -> Result<PathBuf, ResourceError> {
    let portable = path.replace('\\', "/");
    let mut normalized = PathBuf::new();
    for component in Path::new(&portable).components() {
        match component {
            Component::Normal(segment) => normalized.push(segment),
            Component::CurDir => {}
            Component::ParentDir if normalized.pop() => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ResourceError::InvalidRelativePath(path.to_string()));
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(ResourceError::InvalidRelativePath(path.to_string()));
    }
    Ok(normalized)
}
fn path_string(path: &Path) -> Result<String, ResourceError> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| ResourceError::NonUtf8Path(path.to_path_buf()))
}

fn io_error(path: &Path, source: io::Error) -> ResourceError {
    ResourceError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(unix)]
fn metadata_identity(metadata: &Metadata) -> Result<(u64, u64), ResourceError> {
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn metadata_identity(_metadata: &Metadata) -> Result<(u64, u64), ResourceError> {
    Err(ResourceError::UnsupportedPlatform)
}

#[cfg(unix)]
fn same_file_snapshot(left: &Metadata, right: &Metadata) -> Result<bool, ResourceError> {
    Ok(metadata_identity(left)? == metadata_identity(right)?
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec())
}

#[cfg(not(unix))]
fn same_file_snapshot(_left: &Metadata, _right: &Metadata) -> Result<bool, ResourceError> {
    Err(ResourceError::UnsupportedPlatform)
}

#[cfg(unix)]
fn filesystem_identity(metadata: &Metadata) -> Result<String, ResourceError> {
    Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn filesystem_identity(_metadata: &Metadata) -> Result<String, ResourceError> {
    Err(ResourceError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_identity_and_entry_identity_cover_aliases_and_atomic_replace() {
        let directory = tempfile::tempdir().expect("temporary workspace should exist");
        fs::write(directory.path().join("a.txt"), b"one").expect("fixture should write");
        fs::hard_link(
            directory.path().join("a.txt"),
            directory.path().join("alias.txt"),
        )
        .expect("hardlink should be supported");
        fs::create_dir(directory.path().join("tree")).expect("tree should exist");
        fs::write(directory.path().join("tree/child.txt"), b"child").expect("child should write");

        let resolver =
            ResourceResolver::new("workspace", directory.path()).expect("workspace should resolve");
        let original = resolver
            .existing_file("a.txt")
            .expect("file should resolve");
        let alias = resolver
            .existing_file("alias.txt")
            .expect("hardlink should resolve");
        let original_object = original
            .resources()
            .iter()
            .find(|resource| resource.kind == ResourceKind::Object)
            .expect("object key should exist");
        let alias_object = alias
            .resources()
            .iter()
            .find(|resource| resource.kind == ResourceKind::Object)
            .expect("alias object key should exist");
        assert_eq!(original_object.resource_id, alias_object.resource_id);
        assert!(original.overlaps(&alias));

        let tree = resolver
            .directory_tree("tree")
            .expect("directory should resolve");
        let child = resolver
            .existing_file("tree/child.txt")
            .expect("child should resolve");
        assert!(tree.overlaps(&child));

        let original_entry = original
            .resources()
            .iter()
            .find(|resource| resource.kind == ResourceKind::Entry)
            .expect("entry key should exist")
            .resource_id
            .clone();
        fs::write(directory.path().join("replacement"), b"two").expect("replacement should write");
        fs::rename(
            directory.path().join("replacement"),
            directory.path().join("a.txt"),
        )
        .expect("atomic replace should succeed");
        let replaced = resolver
            .existing_file("a.txt")
            .expect("replacement should resolve");
        let replaced_object = replaced
            .resources()
            .iter()
            .find(|resource| resource.kind == ResourceKind::Object)
            .expect("replacement object key should exist");
        let replaced_entry = replaced
            .resources()
            .iter()
            .find(|resource| resource.kind == ResourceKind::Entry)
            .expect("replacement entry key should exist");
        assert_ne!(original_object.resource_id, replaced_object.resource_id);
        assert_eq!(original_entry, replaced_entry.resource_id);
        let observed = resolver
            .observe_existing_file("a.txt")
            .expect("replacement should be observed");
        assert!(observed.iter().any(|observation| matches!(
            observation,
            ResourceObservation::Object {
                observed: ObjectState::Present { blake3, .. },
                ..
            } if *blake3 == digest_bytes(b"two")
        )));
    }

    #[cfg(unix)]
    #[test]
    fn resolver_rejects_parent_and_symlink_workspace_escape() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().expect("temporary workspace should exist");
        let outside = tempfile::tempdir().expect("outside directory should exist");
        fs::write(outside.path().join("outside.txt"), b"outside")
            .expect("outside fixture should write");
        symlink(
            outside.path().join("outside.txt"),
            workspace.path().join("escape.txt"),
        )
        .expect("symlink fixture should write");
        let resolver =
            ResourceResolver::new("workspace", workspace.path()).expect("workspace should resolve");

        assert!(matches!(
            resolver.existing_file("../outside.txt"),
            Err(ResourceError::InvalidRelativePath(_))
        ));
        assert!(matches!(
            resolver.existing_file("escape.txt"),
            Err(ResourceError::SymlinkFile(_))
        ));
    }

    #[test]
    fn entry_identity_changes_when_its_physical_parent_is_replaced() {
        let workspace = tempfile::tempdir().expect("temporary workspace should exist");
        fs::create_dir(workspace.path().join("parent")).expect("parent should exist");
        let resolver =
            ResourceResolver::new("workspace", workspace.path()).expect("workspace should resolve");
        let before = resolver
            .absent_entry("parent/new.txt")
            .expect("absent entry should resolve")
            .into_resources();
        fs::rename(
            workspace.path().join("parent"),
            workspace.path().join("old-parent"),
        )
        .expect("parent should rename");
        fs::create_dir(workspace.path().join("parent")).expect("replacement parent should exist");
        let after = resolver
            .absent_entry("parent/new.txt")
            .expect("replacement entry should resolve")
            .into_resources();
        assert_ne!(before[0].resource_id, after[0].resource_id);
    }

    #[test]
    fn canonical_json_digest_ignores_object_key_order_only() {
        let left = serde_json::json!({ "b": 2, "a": [1, true] });
        let reordered = serde_json::json!({ "a": [1, true], "b": 2 });
        let changed = serde_json::json!({ "a": [true, 1], "b": 2 });

        assert_eq!(
            digest_canonical_json(&left),
            digest_canonical_json(&reordered)
        );
        assert_ne!(
            digest_canonical_json(&left),
            digest_canonical_json(&changed)
        );
    }

    #[test]
    fn operation_start_observations_cover_every_filesystem_operation() {
        let workspace = tempfile::tempdir().expect("temporary workspace should exist");
        fs::write(workspace.path().join("file.txt"), b"file").expect("file should exist");
        fs::write(workspace.path().join("source.txt"), b"source").expect("source should exist");
        fs::create_dir(workspace.path().join("empty")).expect("empty directory should exist");
        fs::create_dir(workspace.path().join("tree")).expect("tree should exist");
        fs::write(workspace.path().join("tree/child.txt"), b"child")
            .expect("tree child should exist");
        let resolver =
            ResourceResolver::new("workspace", workspace.path()).expect("workspace should resolve");

        let operations = vec![
            (
                MutationOperation::Create {
                    path: "new.txt".into(),
                },
                1,
            ),
            (
                MutationOperation::Update {
                    path: "file.txt".into(),
                },
                2,
            ),
            (
                MutationOperation::Delete {
                    path: "file.txt".into(),
                    entry_only: false,
                },
                2,
            ),
            (
                MutationOperation::Rename {
                    old_path: "source.txt".into(),
                    new_path: "rename.txt".into(),
                    entry_only: false,
                },
                3,
            ),
            (
                MutationOperation::Move {
                    old_path: "source.txt".into(),
                    new_path: "move.txt".into(),
                    entry_only: false,
                },
                3,
            ),
            (
                MutationOperation::Hardlink {
                    old_path: "source.txt".into(),
                    new_path: "link.txt".into(),
                },
                3,
            ),
            (
                MutationOperation::Mkdir {
                    path: "new-dir".into(),
                },
                1,
            ),
            (
                MutationOperation::Rmdir {
                    path: "empty".into(),
                },
                2,
            ),
            (
                MutationOperation::WriteDirectory {
                    path: "tree".into(),
                },
                1,
            ),
        ];
        for (operation, expected_count) in operations {
            let observations = resolver
                .observe_operation(&operation)
                .expect("operation should observe physically");
            assert_eq!(observations.len(), expected_count);
            validate_operation_start(&operation, &observations)
                .expect("operation start observations should validate");
        }
    }

    #[test]
    fn operation_transition_rebinds_updates_and_requires_destination_entries() {
        let workspace = tempfile::tempdir().expect("temporary workspace should exist");
        fs::write(workspace.path().join("update.txt"), b"before")
            .expect("update fixture should exist");
        fs::write(workspace.path().join("in-place.txt"), b"before-in-place")
            .expect("in-place update fixture should exist");
        fs::write(workspace.path().join("source.txt"), b"source")
            .expect("rename fixture should exist");
        let resolver =
            ResourceResolver::new("workspace", workspace.path()).expect("workspace should resolve");

        let update = MutationOperation::Update {
            path: "update.txt".into(),
        };
        let update_start = resolver
            .observe_operation(&update)
            .expect("update start should observe");
        assert!(matches!(
            validate_operation_transition(&update, &update_start, &update_start, &update_start),
            Err(ResourceError::InvalidOperationTransition(_))
        ));
        fs::write(workspace.path().join("replacement.txt"), b"after")
            .expect("replacement should exist");
        fs::rename(
            workspace.path().join("replacement.txt"),
            workspace.path().join("update.txt"),
        )
        .expect("replacement should atomically rename");
        let update_post = resolver
            .observe_existing_file("update.txt")
            .expect("replacement should observe");
        validate_operation_transition(&update, &update_start, &update_post, &update_post)
            .expect("atomic replacement should rebind object identity");

        let in_place = MutationOperation::Update {
            path: "in-place.txt".into(),
        };
        let in_place_start = resolver
            .observe_operation(&in_place)
            .expect("in-place update start should observe");
        fs::write(workspace.path().join("in-place.txt"), b"after-in-place")
            .expect("in-place update should apply");
        let in_place_post = resolver
            .observe_existing_file("in-place.txt")
            .expect("in-place post-state should observe");
        let start_object = in_place_start
            .iter()
            .find(|observation| matches!(observation, ResourceObservation::Object { .. }))
            .expect("start object should exist");
        let post_object = in_place_post
            .iter()
            .find(|observation| matches!(observation, ResourceObservation::Object { .. }))
            .expect("post object should exist");
        assert_eq!(
            start_object.resource().resource_id,
            post_object.resource().resource_id
        );
        validate_operation_transition(&in_place, &in_place_start, &in_place_post, &in_place_post)
            .expect("in-place content change should validate");

        let rename = MutationOperation::Rename {
            old_path: "source.txt".into(),
            new_path: "destination.txt".into(),
            entry_only: false,
        };
        let rename_start = resolver
            .observe_operation(&rename)
            .expect("rename start should observe");
        assert!(matches!(
            validate_operation_start(&rename, &rename_start[..2]),
            Err(ResourceError::InvalidOperationObservation(_))
        ));
        fs::rename(
            workspace.path().join("source.txt"),
            workspace.path().join("destination.txt"),
        )
        .expect("source should rename");
        let mut rename_post = resolver
            .observe_existing_file("destination.txt")
            .expect("destination should observe");
        rename_post.extend(
            resolver
                .observe_operation(&MutationOperation::Create {
                    path: "source.txt".into(),
                })
                .expect("source absence should observe"),
        );
        validate_operation_transition(&rename, &rename_start, &rename_post, &rename_post)
            .expect("rename should preserve the object and switch exact entries");
    }

    #[test]
    fn remaining_operation_transitions_accept_exact_physical_post_states() {
        let workspace = tempfile::tempdir().expect("temporary workspace should exist");
        fs::create_dir(workspace.path().join("write-tree")).expect("tree should exist");
        fs::write(workspace.path().join("write-tree/file.txt"), b"before")
            .expect("tree file should exist");
        fs::write(workspace.path().join("delete.txt"), b"delete")
            .expect("delete file should exist");
        fs::write(workspace.path().join("move-source.txt"), b"move")
            .expect("move source should exist");
        fs::write(workspace.path().join("link-source.txt"), b"link")
            .expect("hardlink source should exist");
        fs::create_dir(workspace.path().join("remove-dir")).expect("directory should exist");
        let resolver =
            ResourceResolver::new("workspace", workspace.path()).expect("workspace should resolve");

        let create = MutationOperation::Create {
            path: "create.txt".into(),
        };
        let create_start = resolver
            .observe_operation(&create)
            .expect("create start should observe");
        fs::write(workspace.path().join("create.txt"), b"created").expect("create should apply");
        let create_post = resolver
            .observe_existing_file("create.txt")
            .expect("created file should observe");
        validate_operation_transition(&create, &create_start, &create_post, &create_post)
            .expect("create post-state should validate");

        let delete = MutationOperation::Delete {
            path: "delete.txt".into(),
            entry_only: false,
        };
        let delete_start = resolver
            .observe_operation(&delete)
            .expect("delete start should observe");
        fs::remove_file(workspace.path().join("delete.txt")).expect("delete should apply");
        let delete_post = resolver
            .observe_operation(&MutationOperation::Create {
                path: "delete.txt".into(),
            })
            .expect("deleted entry should observe absent");
        validate_operation_transition(&delete, &delete_start, &delete_post, &delete_post)
            .expect("delete post-state should validate");

        let mkdir = MutationOperation::Mkdir {
            path: "made-dir".into(),
        };
        let mkdir_start = resolver
            .observe_operation(&mkdir)
            .expect("mkdir start should observe");
        fs::create_dir(workspace.path().join("made-dir")).expect("mkdir should apply");
        let mkdir_post = resolver
            .observe_operation(&MutationOperation::Rmdir {
                path: "made-dir".into(),
            })
            .expect("created directory should observe");
        validate_operation_transition(&mkdir, &mkdir_start, &mkdir_post, &mkdir_post)
            .expect("mkdir post-state should validate");

        let rmdir = MutationOperation::Rmdir {
            path: "remove-dir".into(),
        };
        let rmdir_start = resolver
            .observe_operation(&rmdir)
            .expect("rmdir start should observe");
        fs::remove_dir(workspace.path().join("remove-dir")).expect("rmdir should apply");
        let rmdir_post = resolver
            .observe_operation(&MutationOperation::Create {
                path: "remove-dir".into(),
            })
            .expect("removed directory entry should observe absent");
        validate_operation_transition(&rmdir, &rmdir_start, &rmdir_post, &rmdir_post)
            .expect("rmdir post-state should validate");

        let write_directory = MutationOperation::WriteDirectory {
            path: "write-tree".into(),
        };
        let write_directory_start = resolver
            .observe_operation(&write_directory)
            .expect("write-directory start should observe");
        fs::write(workspace.path().join("write-tree/file.txt"), b"after")
            .expect("tree update should apply");
        let write_directory_post = resolver
            .observe_directory_tree("write-tree")
            .expect("updated tree should observe");
        validate_operation_transition(
            &write_directory,
            &write_directory_start,
            &write_directory_post,
            &write_directory_post,
        )
        .expect("write-directory post-state should validate");

        let move_operation = MutationOperation::Move {
            old_path: "move-source.txt".into(),
            new_path: "move-destination.txt".into(),
            entry_only: false,
        };
        let move_start = resolver
            .observe_operation(&move_operation)
            .expect("move start should observe");
        fs::rename(
            workspace.path().join("move-source.txt"),
            workspace.path().join("move-destination.txt"),
        )
        .expect("move should apply");
        let mut move_post = resolver
            .observe_existing_file("move-destination.txt")
            .expect("move destination should observe");
        move_post.extend(
            resolver
                .observe_operation(&MutationOperation::Create {
                    path: "move-source.txt".into(),
                })
                .expect("move source absence should observe"),
        );
        validate_operation_transition(&move_operation, &move_start, &move_post, &move_post)
            .expect("move post-state should validate");

        let hardlink = MutationOperation::Hardlink {
            old_path: "link-source.txt".into(),
            new_path: "link-destination.txt".into(),
        };
        let hardlink_start = resolver
            .observe_operation(&hardlink)
            .expect("hardlink start should observe");
        fs::hard_link(
            workspace.path().join("link-source.txt"),
            workspace.path().join("link-destination.txt"),
        )
        .expect("hardlink should apply");
        let mut hardlink_post = resolver
            .observe_existing_file("link-source.txt")
            .expect("hardlink source should observe");
        hardlink_post.extend(
            resolver
                .observe_existing_file("link-destination.txt")
                .expect("hardlink destination should observe")
                .into_iter()
                .filter(|observation| matches!(observation, ResourceObservation::Entry { .. })),
        );
        validate_operation_transition(&hardlink, &hardlink_start, &hardlink_post, &hardlink_post)
            .expect("hardlink post-state should validate");
    }

    #[test]
    fn structured_commit_requires_adapter_metadata_and_a_changed_git_snapshot() {
        let workspace = tempfile::tempdir().expect("temporary workspace should exist");
        fs::write(workspace.path().join("tracked.txt"), b"tracked")
            .expect("tracked file should exist");
        fs::create_dir(workspace.path().join(".git")).expect("metadata directory should exist");
        fs::write(workspace.path().join(".git/HEAD"), b"initial").expect("metadata should exist");
        let resolver =
            ResourceResolver::new("workspace", workspace.path()).expect("workspace should resolve");
        let operation = MutationOperation::StructuredCommit {
            tracked_paths: vec!["tracked.txt".into(), "deleted.txt".into()],
        };

        assert!(matches!(
            resolver.observe_operation(&operation),
            Err(ResourceError::AdapterSuppliedObservation(_))
        ));
        let mut start = resolver
            .observe_existing_file("tracked.txt")
            .expect("tracked file should observe");
        start.extend(
            resolver
                .observe_operation(&MutationOperation::Create {
                    path: "deleted.txt".into(),
                })
                .expect("deleted tracked path should observe absent"),
        );
        start.extend(
            resolver
                .observe_directory_tree(".git")
                .expect("logical metadata observation should be accepted"),
        );
        validate_operation_start(&operation, &start)
            .expect("adapter-supplied commit metadata should validate");
        let without_git = start
            .iter()
            .filter(|observation| observation.resource().kind != ResourceKind::DirectoryTree)
            .cloned()
            .collect::<Vec<_>>();
        assert!(matches!(
            validate_operation_start(&operation, &without_git),
            Err(ResourceError::InvalidOperationObservation(_))
        ));
        assert!(matches!(
            validate_operation_transition(&operation, &start, &start, &start),
            Err(ResourceError::InvalidOperationTransition(_))
        ));
        fs::write(workspace.path().join(".git/HEAD"), b"changed").expect("metadata should change");
        let mut expected = resolver
            .observe_existing_file("tracked.txt")
            .expect("tracked post-state should observe");
        expected.extend(
            resolver
                .observe_operation(&MutationOperation::Create {
                    path: "deleted.txt".into(),
                })
                .expect("deleted tracked post-state should remain absent"),
        );
        expected.extend(
            resolver
                .observe_directory_tree(".git")
                .expect("changed metadata should observe"),
        );
        for observation in &mut start {
            match observation {
                ResourceObservation::Object { generation, .. }
                | ResourceObservation::Entry { generation, .. }
                | ResourceObservation::DirectoryTree { generation, .. } => *generation = 7,
            }
        }
        let mut actual = expected.clone();
        for observation in &mut actual {
            match observation {
                ResourceObservation::Object { generation, .. }
                | ResourceObservation::Entry { generation, .. }
                | ResourceObservation::DirectoryTree { generation, .. } => *generation = 9,
            }
        }
        validate_operation_transition(&operation, &start, &expected, &actual)
            .expect("projection generations must not change physical transition equality");
    }

    #[cfg(unix)]
    #[test]
    fn entry_only_symlink_delete_and_move_never_follow_the_target() {
        use std::os::unix::{ffi::OsStrExt, fs::symlink};

        let workspace = tempfile::tempdir().expect("temporary workspace should exist");
        let outside = tempfile::tempdir().expect("outside directory should exist");
        let target = outside.path().join("target.txt");
        fs::write(&target, b"outside").expect("outside target should exist");
        symlink(&target, workspace.path().join("link.txt")).expect("symlink should exist");
        let resolver =
            ResourceResolver::new("workspace", workspace.path()).expect("workspace should resolve");

        assert!(matches!(
            resolver.observe_operation(&MutationOperation::Update {
                path: "link.txt".into(),
            }),
            Err(ResourceError::SymlinkFile(_))
        ));
        let delete = MutationOperation::Delete {
            path: "link.txt".into(),
            entry_only: true,
        };
        let delete_start = resolver
            .observe_operation(&delete)
            .expect("symlink delete should observe the link itself");
        assert!(delete_start.iter().any(|observation| matches!(
            observation,
            ResourceObservation::Object {
                observed: ObjectState::Present {
                    kind: ObjectKind::Symlink,
                    blake3,
                    ..
                },
                ..
            } if *blake3 == digest_bytes(target.as_os_str().as_bytes())
        )));
        fs::remove_file(workspace.path().join("link.txt")).expect("symlink should delete");
        assert_eq!(
            fs::read(&target).expect("outside target should remain readable"),
            b"outside"
        );
        let delete_post = resolver
            .observe_operation(&MutationOperation::Create {
                path: "link.txt".into(),
            })
            .expect("deleted symlink entry should be absent");
        validate_operation_transition(&delete, &delete_start, &delete_post, &delete_post)
            .expect("symlink deletion should validate");

        symlink(&target, workspace.path().join("link.txt")).expect("symlink should recreate");
        let move_operation = MutationOperation::Move {
            old_path: "link.txt".into(),
            new_path: "moved-link.txt".into(),
            entry_only: true,
        };
        let move_start = resolver
            .observe_operation(&move_operation)
            .expect("symlink move should observe the link itself");
        fs::rename(
            workspace.path().join("link.txt"),
            workspace.path().join("moved-link.txt"),
        )
        .expect("symlink should move");
        assert_eq!(
            fs::read(&target).expect("outside target should remain readable"),
            b"outside"
        );
        let mut move_post = resolver
            .observe_existing_symlink_entry("moved-link.txt")
            .expect("moved symlink should observe");
        move_post.extend(
            resolver
                .observe_operation(&MutationOperation::Create {
                    path: "link.txt".into(),
                })
                .expect("symlink source entry should be absent"),
        );
        validate_operation_transition(&move_operation, &move_start, &move_post, &move_post)
            .expect("symlink move should validate");
    }

    #[cfg(unix)]
    #[test]
    fn directory_tree_rejects_symlinks_and_hardlinked_regular_files() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().expect("temporary workspace should exist");
        fs::create_dir(workspace.path().join("symlink-tree")).expect("tree should exist");
        fs::write(workspace.path().join("target.txt"), b"target").expect("target should exist");
        symlink(
            workspace.path().join("target.txt"),
            workspace.path().join("symlink-tree/link.txt"),
        )
        .expect("symlink should exist");
        fs::create_dir(workspace.path().join("hardlink-tree")).expect("tree should exist");
        fs::write(workspace.path().join("hardlink-tree/file.txt"), b"file")
            .expect("file should exist");
        fs::hard_link(
            workspace.path().join("hardlink-tree/file.txt"),
            workspace.path().join("hardlink-tree/alias.txt"),
        )
        .expect("hardlink should exist");
        let resolver =
            ResourceResolver::new("workspace", workspace.path()).expect("workspace should resolve");

        assert!(matches!(
            resolver.observe_operation(&MutationOperation::WriteDirectory {
                path: "symlink-tree".into(),
            }),
            Err(ResourceError::SymlinkInDirectoryTree(_))
        ));
        assert!(matches!(
            resolver.observe_operation(&MutationOperation::WriteDirectory {
                path: "hardlink-tree".into(),
            }),
            Err(ResourceError::HardlinkedRegularFile(_))
        ));
    }
}
