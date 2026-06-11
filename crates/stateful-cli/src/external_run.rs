use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Context;

use crate::{
    GlobalPaths,
    sandbox::{SandboxCommandResult, SandboxNetworkPolicy, run_external_sandboxed_command},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalRunRequest {
    pub repo_root: PathBuf,
    pub paths: GlobalPaths,
    pub purpose: String,
    pub command: String,
    pub write_targets: Vec<String>,
    pub create_targets: Vec<String>,
    pub write_dirs: Vec<String>,
    pub network: SandboxNetworkPolicy,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalRunApproval {
    pub request_id: String,
    pub guidance: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
struct StoredExternalRunRequest {
    request_id: String,
    repo_root: String,
    purpose: String,
    command: String,
    write_targets: Vec<String>,
    create_targets: Vec<String>,
    write_dirs: Vec<String>,
    network: SandboxNetworkPolicy,
    timeout_seconds: Option<u64>,
    created_at_unix: u64,
    approved_at_unix: Option<u64>,
    used_at_unix: Option<u64>,
}

pub fn request_external_run(request: ExternalRunRequest) -> anyhow::Result<ExternalRunApproval> {
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
    if write_targets.is_empty() && create_targets.is_empty() && write_dirs.is_empty() {
        anyhow::bail!(
            "external-run requires at least one write target, create target, or write dir"
        );
    }

    let stored = StoredExternalRunRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        repo_root: repo_root.to_string_lossy().to_string(),
        purpose: purpose.to_string(),
        command: request.command,
        write_targets,
        create_targets,
        write_dirs,
        network: request.network,
        timeout_seconds: request.timeout_seconds,
        created_at_unix: now_unix()?,
        approved_at_unix: None,
        used_at_unix: None,
    };
    write_request(&request.paths, &stored)?;

    Ok(ExternalRunApproval {
        request_id: stored.request_id.clone(),
        guidance: approval_required_guidance(&stored)?,
    })
}

pub fn approve_external_run(
    paths: &GlobalPaths,
    request_id: &str,
    run: bool,
) -> anyhow::Result<ExternalRunApproval> {
    let mut request = read_request(paths, request_id)?;
    if request.used_at_unix.is_some() {
        anyhow::bail!("external-run request `{request_id}` has already been used");
    }
    if request.approved_at_unix.is_none() {
        request.approved_at_unix = Some(now_unix()?);
        write_request(paths, &request)?;
    }

    let guidance = if run {
        "Approval recorded. Running the approved external command now.".to_string()
    } else {
        approved_guidance(&request)?
    };
    Ok(ExternalRunApproval {
        request_id: request.request_id,
        guidance,
    })
}

pub fn run_approved_external_run(
    paths: &GlobalPaths,
    request_id: &str,
) -> anyhow::Result<SandboxCommandResult> {
    let mut request = read_request(paths, request_id)?;
    if request.approved_at_unix.is_none() {
        anyhow::bail!("external-run request `{request_id}` has not been approved");
    }
    if request.used_at_unix.is_some() {
        anyhow::bail!("external-run request `{request_id}` has already been used");
    }

    let timeout = Duration::from_secs(request.timeout_seconds.unwrap_or(300).max(1));
    let output = run_external_sandboxed_command(
        &request.command,
        Path::new(&request.repo_root),
        &paths_from_strings(&request.write_targets),
        &paths_from_strings(&request.create_targets),
        &paths_from_strings(&request.write_dirs),
        request.network,
        timeout,
    )?;
    request.used_at_unix = Some(now_unix()?);
    write_request(paths, &request)?;
    Ok(output)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExternalTargetKind {
    ExistingFile,
    CreatableFile,
    ExistingDirectory,
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

fn request_dir(paths: &GlobalPaths) -> PathBuf {
    paths.home.join("external-run").join("requests")
}

fn request_path(paths: &GlobalPaths, request_id: &str) -> anyhow::Result<PathBuf> {
    validate_request_id(request_id)?;
    Ok(request_dir(paths).join(format!("{request_id}.json")))
}

fn read_request(paths: &GlobalPaths, request_id: &str) -> anyhow::Result<StoredExternalRunRequest> {
    let path = request_path(paths, request_id)?;
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("failed to read external-run request {}", path.display()))?;
    let request = serde_json::from_str::<StoredExternalRunRequest>(&contents)
        .with_context(|| format!("failed to parse external-run request {}", path.display()))?;
    if request.request_id != request_id {
        anyhow::bail!("external-run request id mismatch");
    }
    Ok(request)
}

fn write_request(paths: &GlobalPaths, request: &StoredExternalRunRequest) -> anyhow::Result<()> {
    let dir = request_dir(paths);
    fs::create_dir_all(&dir).with_context(|| {
        format!(
            "failed to create external-run request dir {}",
            dir.display()
        )
    })?;
    let path = request_path(paths, &request.request_id)?;
    let contents = serde_json::to_vec_pretty(request)?;
    fs::write(&path, contents)
        .with_context(|| format!("failed to write external-run request {}", path.display()))
}

fn validate_request_id(request_id: &str) -> anyhow::Result<()> {
    if request_id.is_empty()
        || request_id
            .chars()
            .any(|ch| !(ch.is_ascii_alphanumeric() || ch == '-'))
    {
        anyhow::bail!("external-run request id is invalid");
    }
    Ok(())
}

fn approval_required_guidance(request: &StoredExternalRunRequest) -> anyhow::Result<String> {
    Ok(format!(
        "\
External run approval required.

External run request details:
{details}

Copy and paste this command in a trusted terminal to approve and run:
  {binary} external-run approve {request_id} --run
",
        details = format_request_details(request),
        binary = shell_quote(&current_binary()),
        request_id = request.request_id,
    ))
}

fn approved_guidance(request: &StoredExternalRunRequest) -> anyhow::Result<String> {
    Ok(format!(
        "\
External run request approved.

Copy and paste this command to run it:
  {binary} external-run run {request_id}
",
        binary = shell_quote(&current_binary()),
        request_id = request.request_id,
    ))
}

fn format_request_details(request: &StoredExternalRunRequest) -> String {
    let mut lines = vec![format!(
        "  --purpose: {}",
        approval_detail(&request.purpose)
    )];
    for path in &request.write_targets {
        lines.push(format!("  --write-target: {path}"));
    }
    for path in &request.create_targets {
        lines.push(format!("  --create-target: {path}"));
    }
    for path in &request.write_dirs {
        lines.push(format!("  --write-dir: {path}"));
    }
    lines.push(format!(
        "  --command: {}",
        approval_detail(&request.command)
    ));
    lines.join("\n")
}

fn approval_detail(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| format!("{value:?}"))
}

fn paths_from_strings(paths: &[String]) -> Vec<PathBuf> {
    paths.iter().map(PathBuf::from).collect()
}

fn current_binary() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.to_str().map(str::to_owned))
        .unwrap_or_else(|| "stateful".to_string())
}

fn shell_quote(value: &str) -> String {
    let mut quoted = String::from("'");
    for ch in value.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

fn now_unix() -> anyhow::Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}
