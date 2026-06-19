use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;

use anyhow::Context;

use crate::sandbox::{SandboxCommandResult, SandboxNetworkPolicy, run_external_sandboxed_command};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalRunRequest {
    pub repo_root: PathBuf,
    pub purpose: String,
    pub command: String,
    pub write_targets: Vec<String>,
    pub create_targets: Vec<String>,
    pub write_dirs: Vec<String>,
    pub connect_sockets: Vec<String>,
    pub allow_signal: bool,
    pub network: SandboxNetworkPolicy,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidatedExternalRunRequest {
    repo_root: String,
    command: String,
    write_targets: Vec<String>,
    create_targets: Vec<String>,
    write_dirs: Vec<String>,
    connect_sockets: Vec<String>,
    allow_signal: bool,
    network: SandboxNetworkPolicy,
    timeout_seconds: Option<u64>,
}

pub fn request_external_run(request: ExternalRunRequest) -> anyhow::Result<SandboxCommandResult> {
    let request = validate_external_run_request(request)?;
    let timeout = Duration::from_secs(request.timeout_seconds.unwrap_or(300).max(1));
    run_external_sandboxed_command(
        &request.command,
        Path::new(&request.repo_root),
        &paths_from_strings(&request.write_targets),
        &paths_from_strings(&request.create_targets),
        &paths_from_strings(&request.write_dirs),
        &paths_from_strings(&request.connect_sockets),
        request.allow_signal,
        request.network,
        timeout,
    )
}

fn validate_external_run_request(
    request: ExternalRunRequest,
) -> anyhow::Result<ValidatedExternalRunRequest> {
    let purpose = request.purpose.trim();
    if purpose.is_empty() {
        anyhow::bail!("external-run purpose is required");
    }
    if request.command.trim().is_empty() {
        anyhow::bail!("external-run command is required");
    }

    let repo_root = request
        .repo_root
        .canonicalize()
        .map_err(|error| anyhow::anyhow!("external-run repo root must exist: {error}"))?;
    if !repo_root.is_dir() {
        anyhow::bail!("external-run repo root must be a directory");
    }

    let write_targets = normalize_external_targets(
        &repo_root,
        "write target",
        &request.write_targets,
        ExternalTargetKind::ExistingFile,
    )?;
    let create_targets = normalize_external_targets(
        &repo_root,
        "create target",
        &request.create_targets,
        ExternalTargetKind::CreatableFile,
    )?;
    let write_dirs = normalize_external_targets(
        &repo_root,
        "write dir",
        &request.write_dirs,
        ExternalTargetKind::ExistingDirectory,
    )?;
    let connect_sockets = normalize_external_targets(
        &repo_root,
        "connect socket",
        &request.connect_sockets,
        ExternalTargetKind::ExistingUnixSocket,
    )?;
    if write_targets.is_empty()
        && create_targets.is_empty()
        && write_dirs.is_empty()
        && connect_sockets.is_empty()
        && !request.allow_signal
    {
        anyhow::bail!(
            "external-run requires at least one write target, create target, write dir, connect socket, or signal scope"
        );
    }

    Ok(ValidatedExternalRunRequest {
        repo_root: repo_root.to_string_lossy().to_string(),
        command: request.command,
        write_targets,
        create_targets,
        write_dirs,
        connect_sockets,
        allow_signal: request.allow_signal,
        network: request.network,
        timeout_seconds: request.timeout_seconds,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExternalTargetKind {
    ExistingFile,
    CreatableFile,
    ExistingDirectory,
    ExistingUnixSocket,
}

fn normalize_external_targets(
    repo_root: &Path,
    label: &str,
    targets: &[String],
    kind: ExternalTargetKind,
) -> anyhow::Result<Vec<String>> {
    let mut normalized = Vec::new();
    for target in targets {
        let path = normalize_external_target(repo_root, label, target, kind)?;
        if !normalized.contains(&path) {
            normalized.push(path);
        }
    }
    Ok(normalized)
}

fn normalize_external_target(
    repo_root: &Path,
    label: &str,
    target: &str,
    kind: ExternalTargetKind,
) -> anyhow::Result<String> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        anyhow::bail!("external-run {label} entries must not be empty");
    }
    if trimmed.chars().any(char::is_control) {
        anyhow::bail!("external-run paths must not contain control characters");
    }

    let raw_path = Path::new(trimmed);
    let absolute = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        repo_root.join(raw_path)
    };
    let normalized = match kind {
        ExternalTargetKind::ExistingFile => {
            let canonical = absolute
                .canonicalize()
                .with_context(|| format!("external-run {label} `{trimmed}` must exist"))?;
            if !canonical.is_file() {
                anyhow::bail!("external-run {label} `{trimmed}` must be a file");
            }
            canonical
        }
        ExternalTargetKind::CreatableFile => {
            let Some(file_name) = absolute.file_name() else {
                anyhow::bail!("external-run {label} `{trimmed}` must name a file");
            };
            let Some(parent) = absolute.parent() else {
                anyhow::bail!("external-run {label} `{trimmed}` has no parent directory");
            };
            let canonical_parent = parent
                .canonicalize()
                .with_context(|| format!("external-run {label} `{trimmed}` parent must exist"))?;
            if !canonical_parent.is_dir() {
                anyhow::bail!("external-run {label} `{trimmed}` parent must be a directory");
            }
            let candidate = canonical_parent.join(file_name);
            if let Ok(metadata) = fs::symlink_metadata(&candidate) {
                if metadata.file_type().is_symlink() {
                    anyhow::bail!("external-run refuses symlink file targets");
                }
                if metadata.is_dir() {
                    anyhow::bail!("external-run {label} `{trimmed}` must be a file");
                }
            }
            candidate
        }
        ExternalTargetKind::ExistingDirectory => {
            let canonical = absolute
                .canonicalize()
                .with_context(|| format!("external-run {label} `{trimmed}` must exist"))?;
            if !canonical.is_dir() {
                anyhow::bail!("external-run {label} `{trimmed}` must be a directory");
            }
            canonical
        }
        ExternalTargetKind::ExistingUnixSocket => {
            ensure_no_symlinked_existing_components(&absolute)?;
            let metadata = fs::symlink_metadata(&absolute)
                .with_context(|| format!("external-run {label} `{trimmed}` must exist"))?;
            if metadata.file_type().is_symlink() {
                anyhow::bail!("external-run refuses symlink socket targets");
            }
            #[cfg(unix)]
            {
                if !metadata.file_type().is_socket() {
                    anyhow::bail!("external-run {label} `{trimmed}` must be a Unix socket");
                }
            }
            #[cfg(not(unix))]
            {
                let _ = metadata;
                anyhow::bail!("external-run connect socket is only supported on Unix");
            }
            absolute
                .canonicalize()
                .with_context(|| format!("external-run {label} `{trimmed}` must exist"))?
        }
    };

    if normalized.starts_with(repo_root) {
        anyhow::bail!(
            "external-run {label} `{trimmed}` resolves inside the repo; targets must be outside the repo"
        );
    }

    normalized
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("external-run normalized path is not valid UTF-8"))
}

fn ensure_no_symlinked_existing_components(path: &Path) -> anyhow::Result<()> {
    let mut cursor = PathBuf::new();
    for component in path.components() {
        cursor.push(component.as_os_str());
        match fs::symlink_metadata(&cursor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                anyhow::bail!("external-run refuses symlinked target components");
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }

    Ok(())
}

fn paths_from_strings(paths: &[String]) -> Vec<PathBuf> {
    paths.iter().map(PathBuf::from).collect()
}
