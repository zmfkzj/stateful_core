#[cfg(any(target_os = "macos", test))]
use std::process::Command;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(any(target_os = "macos", test))]
use crate::sandbox::apply_sandbox_temp_env;
#[cfg(target_os = "macos")]
use crate::sandbox::run_command_with_timeout;
use crate::sandbox::{
    STATEFUL_SANDBOX_RUN_ACTIVE_ENV, SandboxAuthorizationDenied, SandboxAuthorizeContext,
    SandboxAuthorizeDecision, SandboxCommandResult, SandboxNetworkPolicy, SandboxRunOutput,
    SandboxWritablePath, SandboxWritablePathKind, authorize_sandbox_write,
    classify_sandbox_authorize_response, enrich_sandbox_write_dir_denial, ensure_repo_dir_target,
    normalize_sandbox_target_path, push_seatbelt_device_read_allows, resolve_sandbox_cwd,
    sandbox_temp_dir, sandbox_write_dir_display_path, seatbelt_escape,
};
use crate::{
    CurrentSession, GlobalPaths, RepoGate, discover_runtime_with_global, ensure_server,
    read_current_session_file, repo_gate, runtime_env_override_is_configured,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NestedCodexBenchmarkSandboxRequest {
    pub purpose: String,
    pub write_dir: String,
    pub codex_home_root: String,
    pub command: String,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NestedCodexBenchmarkSandboxPaths {
    write_dir: PathBuf,
    codex_home_root: PathBuf,
    write_dir_relative: String,
}

pub fn run_nested_codex_benchmark_sandbox_in_repo(
    repo_root: &Path,
    paths: &GlobalPaths,
    request: NestedCodexBenchmarkSandboxRequest,
) -> anyhow::Result<SandboxRunOutput> {
    if request.purpose.trim().is_empty() {
        anyhow::bail!("stateful sandbox run-nested-codex-benchmark requires --purpose");
    }
    if request.command.trim().is_empty() {
        anyhow::bail!("stateful sandbox run-nested-codex-benchmark requires a non-empty --command");
    }
    if std::env::var_os(STATEFUL_SANDBOX_RUN_ACTIVE_ENV).is_some() {
        anyhow::bail!(
            "stateful sandbox run-nested-codex-benchmark must be the outermost sandbox command"
        );
    }

    let repo_root = match repo_gate(paths, repo_root)? {
        RepoGate::Enabled { repo_root } => {
            if !runtime_env_override_is_configured() {
                ensure_server(paths)?;
            }
            repo_root
        }
        RepoGate::Disabled => {
            anyhow::bail!("stateful sandbox run-nested-codex-benchmark requires an enabled repo")
        }
        RepoGate::OutsideGitRepo => {
            anyhow::bail!("stateful sandbox run-nested-codex-benchmark requires a Git repo")
        }
    };
    let runtime = discover_runtime_with_global(&repo_root, paths)?;
    let nested_paths = validate_nested_codex_benchmark_paths(
        &repo_root,
        &request.write_dir,
        &request.codex_home_root,
    )?;

    let current_session: CurrentSession = read_current_session_file(&repo_root).map_err(|_| {
        anyhow::anyhow!("sandbox run-nested-codex-benchmark requires a current stateful session")
    })?;
    let authorize_context = SandboxAuthorizeContext {
        runtime: &runtime,
        repo_root: &repo_root,
        paths,
        session_id: &current_session.session_id,
        workspace_id: &current_session.workspace_id,
        network: SandboxNetworkPolicy::Enabled,
        fs_profile: "nested-codex-benchmark",
    };
    let authorization_path = sandbox_write_dir_display_path(&nested_paths.write_dir_relative);
    let response =
        authorize_sandbox_write(&authorize_context, "write_directory", &authorization_path)?;
    let mut allowed_write_targets = Vec::new();
    let mut denied_write_targets = Vec::new();
    match classify_sandbox_authorize_response(&nested_paths.write_dir_relative, response)? {
        SandboxAuthorizeDecision::Allow => allowed_write_targets.push(authorization_path),
        SandboxAuthorizeDecision::Deny(body) => {
            denied_write_targets.push(serde_json::json!({
                "path": sandbox_write_dir_display_path(&nested_paths.write_dir_relative),
                "authorization": enrich_sandbox_write_dir_denial(body),
            }));
        }
    }
    if !denied_write_targets.is_empty() {
        let body = serde_json::json!({
            "status": "error",
            "message": "stateful sandbox run-nested-codex-benchmark target authorization denied",
            "allowed_write_targets": allowed_write_targets,
            "denied_write_targets": denied_write_targets,
        })
        .to_string();
        return Err(SandboxAuthorizationDenied::new(body).into());
    }

    let writable_paths = prepare_nested_codex_benchmark_writable_paths(&nested_paths)?;
    let cwd = resolve_sandbox_cwd(&repo_root)?;
    let timeout = Duration::from_secs(request.timeout_seconds.unwrap_or(300).max(1));
    let result = run_nested_codex_benchmark_sandboxed_command(
        &request.command,
        &cwd,
        &writable_paths,
        &nested_paths.codex_home_root,
        timeout,
    )?;

    Ok(SandboxRunOutput {
        status: result.status,
        exit_code: result.exit_code,
        stdout: result.stdout,
        stderr: result.stderr,
        allowed_write_targets,
        denied_write_targets: Vec::new(),
    })
}

fn validate_nested_codex_benchmark_paths(
    repo_root: &Path,
    write_dir: &str,
    codex_home_root: &str,
) -> anyhow::Result<NestedCodexBenchmarkSandboxPaths> {
    let write_dir = normalize_sandbox_target_path("write_dirs", write_dir)?;
    if write_dir != "target" {
        anyhow::bail!("stateful sandbox run-nested-codex-benchmark requires --write-dir target");
    }

    let codex_home_root =
        normalize_sandbox_target_path("codex_home_root", codex_home_root).map_err(|error| {
            anyhow::anyhow!(
                "stateful sandbox run-nested-codex-benchmark requires --codex-home-root under target: {error}"
            )
        })?;
    if !codex_home_root.starts_with("target/") {
        anyhow::bail!(
            "stateful sandbox run-nested-codex-benchmark requires --codex-home-root under target"
        );
    }

    ensure_repo_dir_target(repo_root, &write_dir).map_err(|error| {
        anyhow::anyhow!(
            "stateful sandbox run-nested-codex-benchmark write dir `{write_dir}` is unsafe: {error}"
        )
    })?;
    ensure_repo_dir_target(repo_root, &codex_home_root).map_err(|error| {
        anyhow::anyhow!(
            "stateful sandbox run-nested-codex-benchmark codex home root `{codex_home_root}` is unsafe: {error}"
        )
    })?;

    Ok(NestedCodexBenchmarkSandboxPaths {
        write_dir: repo_root.join(&write_dir),
        codex_home_root: repo_root.join(&codex_home_root),
        write_dir_relative: write_dir,
    })
}

fn prepare_nested_codex_benchmark_writable_paths(
    paths: &NestedCodexBenchmarkSandboxPaths,
) -> anyhow::Result<Vec<SandboxWritablePath>> {
    fs::create_dir_all(&paths.write_dir)?;
    fs::create_dir_all(paths.write_dir.join(".stateful-tmp"))?;
    fs::create_dir_all(&paths.codex_home_root)?;

    let mut seen = BTreeSet::new();
    let mut writable_paths = Vec::new();
    for path in [&paths.write_dir, &paths.codex_home_root] {
        let canonical = path.canonicalize()?;
        if seen.insert(canonical.clone()) {
            writable_paths.push(SandboxWritablePath::directory(canonical));
        }
    }

    Ok(writable_paths)
}

fn run_nested_codex_benchmark_sandboxed_command(
    command: &str,
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    codex_home_root: &Path,
    timeout: Duration,
) -> anyhow::Result<SandboxCommandResult> {
    let temp_dir = sandbox_temp_dir(writable_paths)
        .ok_or_else(|| anyhow::anyhow!("nested Codex benchmark sandbox requires a temp dir"))?;
    #[cfg(target_os = "macos")]
    {
        run_command_with_timeout(
            nested_codex_benchmark_seatbelt_command(
                command,
                cwd,
                writable_paths,
                &temp_dir,
                codex_home_root,
            ),
            timeout,
        )
    }

    #[cfg(target_os = "linux")]
    {
        let _ = (
            command,
            cwd,
            writable_paths,
            codex_home_root,
            timeout,
            temp_dir,
        );
        anyhow::bail!(
            "stateful sandbox run-nested-codex-benchmark is currently supported only on macOS"
        );
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (
            command,
            cwd,
            writable_paths,
            codex_home_root,
            timeout,
            temp_dir,
        );
        anyhow::bail!(
            "stateful sandbox run-nested-codex-benchmark is currently supported only on macOS"
        );
    }
}

#[cfg(any(target_os = "macos", test))]
fn apply_nested_codex_benchmark_env(
    command: &mut Command,
    temp_dir: &Path,
    codex_home_root: &Path,
) {
    apply_sandbox_temp_env(command, Some(temp_dir));
    command.env("STATEFUL_NESTED_CODEX_HOME_ROOT", codex_home_root);
}

#[cfg(target_os = "macos")]
fn nested_codex_benchmark_seatbelt_command(
    command: &str,
    cwd: &Path,
    writable_paths: &[SandboxWritablePath],
    temp_dir: &Path,
    codex_home_root: &Path,
) -> Command {
    let profile = nested_codex_benchmark_seatbelt_profile(writable_paths);
    let mut sandbox = Command::new("/usr/bin/sandbox-exec");
    sandbox
        .arg("-p")
        .arg(profile)
        .arg("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd);
    apply_nested_codex_benchmark_env(&mut sandbox, temp_dir, codex_home_root);
    sandbox
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn nested_codex_benchmark_seatbelt_profile(writable_paths: &[SandboxWritablePath]) -> String {
    let mut profile =
        String::from("(version 1)\n(deny default)\n(allow process*)\n(allow file-read*)\n");
    push_seatbelt_device_read_allows(&mut profile);
    profile.push_str(
        "(allow sysctl-read)\n(allow network*)\n(allow file-write* (literal \"/dev/null\")",
    );
    for writable_path in writable_paths {
        profile.push_str(match writable_path.kind {
            SandboxWritablePathKind::File => " (literal \"",
            SandboxWritablePathKind::Directory => " (subpath \"",
        });
        profile.push_str(&seatbelt_escape(&writable_path.path.to_string_lossy()));
        profile.push_str("\")");
    }
    profile.push_str(")\n");
    profile
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_codex_benchmark_paths_must_stay_under_target() {
        let repo_root = Path::new("/repo");
        let paths = validate_nested_codex_benchmark_paths(
            repo_root,
            "target",
            "target/nested-codex-homes/run-1",
        )
        .expect("valid nested Codex benchmark paths should pass");
        assert_eq!(paths.write_dir, PathBuf::from("/repo/target"));
        assert_eq!(
            paths.codex_home_root,
            PathBuf::from("/repo/target/nested-codex-homes/run-1")
        );

        for (write_dir, codex_home_root, expected) in [
            (
                "crates",
                "target/nested-codex-homes/run-1",
                "requires --write-dir target",
            ),
            (
                "target/bench",
                "target/nested-codex-homes/run-1",
                "requires --write-dir target",
            ),
            (
                "target",
                "target/../codex-home",
                "requires --codex-home-root under target",
            ),
        ] {
            let error =
                validate_nested_codex_benchmark_paths(repo_root, write_dir, codex_home_root)
                    .expect_err("invalid nested Codex benchmark paths should fail");
            assert!(
                error.to_string().contains(expected),
                "unexpected error for {write_dir} {codex_home_root}: {error}"
            );
        }
    }

    #[test]
    fn nested_codex_benchmark_env_sets_isolated_codex_home_root() {
        let mut command = Command::new("printenv");
        apply_nested_codex_benchmark_env(
            &mut command,
            Path::new("/repo/target/.stateful-tmp"),
            Path::new("/repo/target/nested-codex-homes/run-1"),
        );

        let env = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().to_string(),
                    value.map(|value| value.to_string_lossy().to_string()),
                )
            })
            .collect::<Vec<_>>();

        assert!(env.contains(&(
            "STATEFUL_SANDBOX_RUN_ACTIVE".to_string(),
            Some("1".to_string())
        )));
        assert_eq!(
            env.iter()
                .find(|(key, _)| key == "STATEFUL_NESTED_CODEX_HOME_ROOT")
                .map(|(_, value)| value),
            Some(&Some("/repo/target/nested-codex-homes/run-1".to_string()))
        );
    }

    #[test]
    fn nested_codex_benchmark_seatbelt_profile_is_target_only_with_network() {
        let profile = nested_codex_benchmark_seatbelt_profile(&[
            SandboxWritablePath::directory(PathBuf::from("/repo/target")),
            SandboxWritablePath::directory(PathBuf::from("/repo/target/nested-codex-homes/run-1")),
        ]);

        assert!(profile.contains("(allow network*)"));
        assert!(profile.contains(
            "(allow file-read* (literal \"/dev/null\") (literal \"/dev/zero\") (literal \"/dev/urandom\"))"
        ));
        let write_rules = profile
            .lines()
            .filter(|line| line.starts_with("(allow file-write*"))
            .collect::<Vec<_>>();
        assert!(
            write_rules
                .iter()
                .any(|line| line.contains("(literal \"/dev/null\")"))
        );
        assert!(!write_rules.iter().any(|line| line.contains("/dev/zero")));
        assert!(!write_rules.iter().any(|line| line.contains("/dev/urandom")));
        assert!(profile.contains("(subpath \"/repo/target\")"));
        assert!(profile.contains("(subpath \"/repo/target/nested-codex-homes/run-1\")"));
        assert!(!profile.contains("(subpath \"/repo\")"));
    }
}
