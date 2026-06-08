use std::{
    net::{IpAddr, UdpSocket},
    path::PathBuf,
};

use clap::Subcommand;

use crate::{
    GlobalPaths, InstallOptions, ServerRuntime, apply_global_install, enable_repo,
    runtime_from_remote, write_global_runtime_file,
};

#[derive(Debug, Subcommand)]
pub enum LanCommand {
    Serve {
        #[arg(long, default_value = "0.0.0.0")]
        host: String,
        #[arg(long, default_value_t = 43873)]
        port: u16,
        #[arg(long)]
        token: Option<String>,
        #[arg(long, default_value = "shared")]
        workspace_id: String,
    },
    Join {
        base_url: String,
        #[arg(long)]
        token: String,
        #[arg(long, default_value = "shared")]
        workspace_id: String,
        #[arg(long)]
        enable_repo: bool,
        #[arg(long)]
        binary: Option<String>,
        #[arg(long)]
        codex_config: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanJoinOptions {
    pub paths: GlobalPaths,
    pub codex_config_path: PathBuf,
    pub binary_path: String,
    pub base_url: String,
    pub token: String,
    pub workspace_id: String,
    pub enable_repo_root: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanJoinResult {
    pub status: String,
    pub runtime: ServerRuntime,
    pub repo_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanServeOptions {
    pub paths: GlobalPaths,
    pub host: String,
    pub port: u16,
    pub token: Option<String>,
    pub workspace_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanServeResult {
    pub runtime: ServerRuntime,
    pub join_commands: Vec<String>,
}

pub fn join_lan_runtime(options: LanJoinOptions) -> anyhow::Result<LanJoinResult> {
    let runtime = runtime_from_remote(&options.base_url, &options.token, &options.workspace_id)?;
    apply_global_install(InstallOptions {
        yes: true,
        paths: options.paths.clone(),
        codex_config_path: options.codex_config_path,
        binary_path: options.binary_path,
    })?;
    write_global_runtime_file(&options.paths, &runtime)?;
    let repo_enabled = if let Some(repo_root) = options.enable_repo_root {
        enable_repo(&options.paths, repo_root, false)?;
        true
    } else {
        false
    };

    Ok(LanJoinResult {
        status: "ok".to_string(),
        runtime,
        repo_enabled,
    })
}

pub fn lan_join_commands(addresses: &[IpAddr], port: u16, token: &str) -> Vec<String> {
    lan_join_commands_with_workspace(addresses, port, token, "shared")
}

fn lan_join_commands_with_workspace(
    addresses: &[IpAddr],
    port: u16,
    token: &str,
    workspace_id: &str,
) -> Vec<String> {
    addresses
        .iter()
        .map(|address| match address {
            IpAddr::V4(address) => {
                format_lan_join_command(&address.to_string(), port, token, workspace_id)
            }
            IpAddr::V6(address) => {
                format_lan_join_command(&format!("[{address}]"), port, token, workspace_id)
            }
        })
        .collect()
}

pub fn serve_lan_runtime(options: LanServeOptions) -> anyhow::Result<LanServeResult> {
    let runtime = crate::ensure_server_with_options(
        &options.paths,
        crate::ServerStartOptions {
            host: options.host.clone(),
            port: options.port,
            token: options.token,
            workspace_id: options.workspace_id,
        },
    )?;
    let addresses = join_addresses_for_host(&options.host, detected_lan_addresses());
    let join_commands = lan_join_commands_with_workspace(
        &addresses,
        options.port,
        &runtime.token,
        &runtime.workspace_id,
    );
    Ok(LanServeResult {
        runtime,
        join_commands,
    })
}

pub(crate) fn run_lan_command(command: LanCommand, paths: GlobalPaths) -> anyhow::Result<()> {
    match command {
        LanCommand::Serve {
            host,
            port,
            token,
            workspace_id,
        } => {
            let result = serve_lan_runtime(LanServeOptions {
                paths,
                host,
                port,
                token,
                workspace_id,
            })?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": "ok",
                    "base_url": result.runtime.base_url,
                    "workspace_id": result.runtime.workspace_id,
                    "join_commands": result.join_commands,
                    "warning": "LAN mode exposes the stateful HTTP runtime on the local network; use SSH tunneling on untrusted networks.",
                }))?
            );
            Ok(())
        }
        LanCommand::Join { .. } => {
            anyhow::bail!("lan join is dispatched by the top-level command runner")
        }
    }
}

fn join_addresses_for_host(host: &str, detected_addresses: Vec<IpAddr>) -> Vec<IpAddr> {
    if let Ok(address) = host.parse::<IpAddr>() {
        if !address.is_unspecified() {
            return vec![address];
        }
    }
    detected_addresses
}

fn format_lan_join_command(address: &str, port: u16, token: &str, workspace_id: &str) -> String {
    let mut command = format!("stateful lan join http://{address}:{port} --token {token}");
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
