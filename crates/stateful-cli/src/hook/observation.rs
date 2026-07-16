use std::path::Path;

use stateful_core::{ContentFingerprint, ReadClassification, fingerprint_path};

use super::input::ToolMetadata;

pub(crate) fn fingerprint(
    repo_root: &Path,
    relative_path: &str,
) -> anyhow::Result<ContentFingerprint> {
    Ok(fingerprint_path(&repo_root.join(relative_path))?)
}

pub(crate) fn classification(metadata: &ToolMetadata) -> ReadClassification {
    if !metadata.successful() {
        ReadClassification::Failed
    } else if metadata.truncated() {
        ReadClassification::Truncated
    } else if !metadata.complete() {
        ReadClassification::Partial
    } else {
        ReadClassification::Exact
    }
}
