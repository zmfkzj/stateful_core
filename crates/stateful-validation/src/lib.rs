use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

pub const CRATE_NAME: &str = "stateful-validation";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ValidationConfig {
    pub profiles: Vec<ValidationProfile>,
}

impl ValidationConfig {
    pub fn from_yaml(input: &str) -> serde_yaml::Result<Self> {
        serde_yaml::from_str(input)
    }

    pub fn profile(&self, profile_id: &str) -> Option<&ValidationProfile> {
        self.profiles
            .iter()
            .find(|profile| profile.profile_id == profile_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ValidationProfile {
    pub profile_id: String,
    pub description: String,
    pub command: String,
    pub cwd: String,
    pub timeout_seconds: u64,
    #[serde(default)]
    pub allowed_writes: Vec<String>,
    #[serde(default)]
    pub denied_writes: Vec<String>,
    #[serde(default)]
    pub exclusive: bool,
    #[serde(default = "default_result_parser")]
    pub result_parser: ResultParser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultParser {
    ExitCode,
}

fn default_result_parser() -> ResultParser {
    ResultParser::ExitCode
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus {
    Passed,
    Failed,
    FailedPolicy,
    Timeout,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidationResult {
    pub profile_id: String,
    pub status: ValidationStatus,
    pub exit_code: Option<i32>,
    pub message: String,
}

pub fn run_validation_profile(
    repo_root: impl AsRef<Path>,
    profile_id: &str,
) -> anyhow::Result<ValidationResult> {
    let repo_root = repo_root.as_ref();
    let config_path = repo_root.join(".stateful/validation.yml");
    let config = ValidationConfig::from_yaml(&std::fs::read_to_string(config_path)?)?;
    let profile = config
        .profile(profile_id)
        .ok_or_else(|| anyhow::anyhow!("validation profile not found: {profile_id}"))?;

    let denied = build_glob_set(&profile.denied_writes)?;
    let before = git_dirty_paths(repo_root)?;
    let dirty_denied_before = matching_paths(&before, &denied);
    if !dirty_denied_before.is_empty() {
        return Ok(ValidationResult {
            profile_id: profile.profile_id.clone(),
            status: ValidationStatus::Error,
            exit_code: None,
            message: format!(
                "denied path dirty before validation: {}",
                dirty_denied_before.join(", ")
            ),
        });
    }

    let output = match run_profile_command(repo_root, profile)? {
        CommandRun::Completed(output) => output,
        CommandRun::TimedOut => {
            return Ok(ValidationResult {
                profile_id: profile.profile_id.clone(),
                status: ValidationStatus::Timeout,
                exit_code: None,
                message: "validation command exceeded timeout".to_string(),
            });
        }
    };

    let after = git_dirty_paths(repo_root)?;
    let new_dirty = after.difference(&before).cloned().collect::<BTreeSet<_>>();
    let dirty_denied_after = matching_paths(&new_dirty, &denied);
    if !dirty_denied_after.is_empty() {
        return Ok(ValidationResult {
            profile_id: profile.profile_id.clone(),
            status: ValidationStatus::FailedPolicy,
            exit_code: output.status.code(),
            message: format!(
                "validation wrote denied path: {}",
                dirty_denied_after.join(", ")
            ),
        });
    }

    if output.status.success() {
        Ok(ValidationResult {
            profile_id: profile.profile_id.clone(),
            status: ValidationStatus::Passed,
            exit_code: output.status.code(),
            message: "validation passed".to_string(),
        })
    } else {
        Ok(ValidationResult {
            profile_id: profile.profile_id.clone(),
            status: ValidationStatus::Failed,
            exit_code: output.status.code(),
            message: "validation command failed".to_string(),
        })
    }
}

fn build_glob_set(patterns: &[String]) -> anyhow::Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern)?);
    }
    Ok(builder.build()?)
}

fn matching_paths(paths: &BTreeSet<String>, glob_set: &GlobSet) -> Vec<String> {
    paths
        .iter()
        .filter(|path| glob_set.is_match(path.as_str()))
        .cloned()
        .collect()
}

fn git_dirty_paths(repo_root: &Path) -> anyhow::Result<BTreeSet<String>> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repo_root)
        .output()?;
    if !output.status.success() {
        anyhow::bail!("git status --porcelain failed");
    }

    let stdout = String::from_utf8(output.stdout)?;
    Ok(stdout
        .lines()
        .filter_map(parse_porcelain_path)
        .collect::<BTreeSet<_>>())
}

fn parse_porcelain_path(line: &str) -> Option<String> {
    if line.len() < 4 {
        return None;
    }

    let path = &line[3..];
    let path = path.split(" -> ").last().unwrap_or(path);
    Some(path.to_string())
}

enum CommandRun {
    Completed(Output),
    TimedOut,
}

fn run_profile_command(
    repo_root: &Path,
    profile: &ValidationProfile,
) -> anyhow::Result<CommandRun> {
    let cwd: PathBuf = repo_root.join(&profile.cwd);
    let timeout = Duration::from_secs(profile.timeout_seconds);
    let started = Instant::now();
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&profile.command)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    loop {
        if child.try_wait()?.is_some() {
            return child
                .wait_with_output()
                .map(CommandRun::Completed)
                .map_err(Into::into);
        }

        if started.elapsed() >= timeout {
            terminate_child_processes(child.id());
            child.kill()?;
            let _ = child.wait();
            return Ok(CommandRun::TimedOut);
        }

        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(unix)]
fn terminate_child_processes(parent_pid: u32) {
    let _ = Command::new("pkill")
        .args(["-TERM", "-P", &parent_pid.to_string()])
        .status();
}

#[cfg(not(unix))]
fn terminate_child_processes(_parent_pid: u32) {}
