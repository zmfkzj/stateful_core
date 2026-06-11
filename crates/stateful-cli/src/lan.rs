use std::{
    net::{IpAddr, UdpSocket},
    path::{Path, PathBuf},
};

use crate::{
    GlobalPaths, InstallOptions, ServerRuntime, apply_global_install, detect_git_root, enable_repo,
    runtime_from_remote, write_global_runtime_file,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerJoinOptions {
    pub paths: GlobalPaths,
    pub codex_config_path: PathBuf,
    pub binary_path: String,
    pub base_url: String,
    pub token: String,
    pub workspace_id: String,
    pub enable_repo_root: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerJoinResult {
    pub status: String,
    pub runtime: ServerRuntime,
    pub repo_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerStartRuntimeOptions {
    pub paths: GlobalPaths,
    pub host: String,
    pub port: u16,
    pub token: Option<String>,
    pub workspace_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerStartRuntimeResult {
    pub runtime: ServerRuntime,
    pub join_commands: Vec<String>,
}

pub fn join_server_runtime(options: ServerJoinOptions) -> anyhow::Result<ServerJoinResult> {
    let runtime = runtime_from_remote(&options.base_url, &options.token, &options.workspace_id)?;
    apply_global_install(InstallOptions {
        yes: true,
        paths: options.paths.clone(),
        codex_config_path: options.codex_config_path,
        binary_path: options.binary_path,
    })?;
    write_global_runtime_file(&options.paths, &runtime)?;
    let repo_enabled = match options.enable_repo_root {
        Some(repo_root) => enable_server_join_repo(
            repo_root,
            |repo_root| detect_git_root(repo_root),
            |git_root| enable_repo(&options.paths, git_root).map(|_| ()),
        )?,
        None => false,
    };

    Ok(ServerJoinResult {
        status: "ok".to_string(),
        runtime,
        repo_enabled,
    })
}

fn enable_server_join_repo<DetectGitRoot, EnableRepo>(
    repo_root: PathBuf,
    detect_git_root: DetectGitRoot,
    enable_repo: EnableRepo,
) -> anyhow::Result<bool>
where
    DetectGitRoot: FnOnce(&Path) -> anyhow::Result<PathBuf>,
    EnableRepo: FnOnce(PathBuf) -> anyhow::Result<()>,
{
    match detect_git_root(&repo_root) {
        Ok(git_root) => {
            enable_repo(git_root)?;
            Ok(true)
        }
        Err(error) if is_no_git_root_error(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

fn is_no_git_root_error(error: &anyhow::Error) -> bool {
    error.to_string().starts_with("no git root found from ")
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use std::{cell::Cell, path::Path};

    #[test]
    fn server_join_repo_enablement_skips_no_git_root_without_enabling() {
        let enable_called = Cell::new(false);

        let repo_enabled = enable_server_join_repo(
            PathBuf::from("outside-git"),
            |_start| anyhow::bail!("no git root found from outside-git"),
            |_git_root| {
                enable_called.set(true);
                Ok(())
            },
        )
        .expect("no git root should skip repo enablement");

        assert!(!repo_enabled);
        assert!(!enable_called.get());
    }

    #[test]
    fn server_join_repo_enablement_propagates_unrelated_detection_error() {
        let enable_called = Cell::new(false);

        let error = enable_server_join_repo(
            PathBuf::from("missing"),
            |_start| anyhow::bail!("failed to canonicalize missing"),
            |_git_root| {
                enable_called.set(true);
                Ok(())
            },
        )
        .expect_err("unrelated detection errors should propagate");

        assert!(error.to_string().contains("failed to canonicalize"));
        assert!(!enable_called.get());
    }

    #[test]
    fn server_join_repo_enablement_enables_detected_git_root() {
        let detected_root = PathBuf::from("/repo");
        let enable_called = Cell::new(false);

        let repo_enabled = enable_server_join_repo(
            PathBuf::from("/repo/nested"),
            |_start: &Path| Ok(detected_root.clone()),
            |git_root| {
                assert_eq!(git_root, detected_root);
                enable_called.set(true);
                Ok(())
            },
        )
        .expect("detected git root should enable repo");

        assert!(repo_enabled);
        assert!(enable_called.get());
    }
}

pub fn server_join_commands(addresses: &[IpAddr], port: u16, token: &str) -> Vec<String> {
    server_join_commands_with_workspace(addresses, port, token, "shared")
}

fn server_join_commands_with_workspace(
    addresses: &[IpAddr],
    port: u16,
    token: &str,
    workspace_id: &str,
) -> Vec<String> {
    addresses
        .iter()
        .map(|address| match address {
            IpAddr::V4(address) => {
                format_server_join_command(&address.to_string(), port, token, workspace_id)
            }
            IpAddr::V6(address) => {
                format_server_join_command(&format!("[{address}]"), port, token, workspace_id)
            }
        })
        .collect()
}

pub fn start_server_runtime(
    options: ServerStartRuntimeOptions,
) -> anyhow::Result<ServerStartRuntimeResult> {
    let runtime = crate::ensure_server_with_options(
        &options.paths,
        crate::ServerStartOptions {
            host: options.host.clone(),
            port: options.port,
            token: options.token,
            workspace_id: options.workspace_id,
        },
    )?;
    Ok(server_start_runtime_result(
        runtime,
        &options.host,
        options.port,
    ))
}

pub fn server_start_runtime_result(
    runtime: ServerRuntime,
    host: &str,
    port: u16,
) -> ServerStartRuntimeResult {
    let addresses = join_addresses_for_host(host, detected_lan_addresses());
    let join_commands = server_join_commands_with_workspace(
        &addresses,
        port,
        &runtime.token,
        &runtime.workspace_id,
    );
    ServerStartRuntimeResult {
        runtime,
        join_commands,
    }
}

pub fn print_server_start_result(result: &ServerStartRuntimeResult) -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "ok",
            "base_url": result.runtime.base_url,
            "workspace_id": result.runtime.workspace_id,
            "join_commands": result.join_commands,
            "warning": "Binding the stateful HTTP runtime to a LAN-reachable host exposes it on the local network; use SSH tunneling on untrusted networks.",
        }))?
    );
    Ok(())
}

fn join_addresses_for_host(host: &str, detected_addresses: Vec<IpAddr>) -> Vec<IpAddr> {
    if let Ok(address) = host.parse::<IpAddr>() {
        if !address.is_unspecified() {
            return vec![address];
        }
    }
    detected_addresses
}

fn format_server_join_command(address: &str, port: u16, token: &str, workspace_id: &str) -> String {
    let mut command = format!("stateful server join http://{address}:{port} --token {token}");
    if workspace_id != "shared" {
        command.push_str(" --workspace-id ");
        command.push_str(workspace_id);
    }
    command
}

fn detected_lan_addresses() -> Vec<IpAddr> {
    let mut addresses = Vec::new();
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(local_addr) = socket.local_addr() {
                let ip = local_addr.ip();
                if !ip.is_loopback() && !ip.is_unspecified() {
                    addresses.push(ip);
                }
            }
        }
    }
    addresses.sort();
    addresses.dedup();
    addresses
}
