use std::{
    collections::BTreeSet,
    path::PathBuf,
    process::{Command as ProcessCommand, Stdio},
};

use clap::ValueEnum;

use crate::runtime::STATEFUL_CODEX_RUN_ID_ENV;

pub const STATEFUL_TRUSTED_SANDBOX_ENV: &str = "STATEFUL_HOOK_TRUSTED_SANDBOX";

const READ_ONLY_TMP_PROFILE: &str = "stateful-read-only-tmp";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CodexSandboxMode {
    Passthrough,
    ReadOnlyTmp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexWrapperOptions {
    pub codex_bin: String,
    pub sandbox: CodexSandboxMode,
    pub no_stateful: bool,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexInvocation {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub fn build_codex_invocation(options: CodexWrapperOptions) -> anyhow::Result<CodexInvocation> {
    match options.sandbox {
        CodexSandboxMode::Passthrough => build_passthrough_invocation(options),
        CodexSandboxMode::ReadOnlyTmp => build_read_only_tmp_invocation(options),
    }
}

pub fn run_codex(options: CodexWrapperOptions) -> anyhow::Result<i32> {
    let invocation = build_codex_invocation(options)?;
    let status = ProcessCommand::new(&invocation.program)
        .args(&invocation.args)
        .envs(invocation.env)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;

    Ok(status.code().unwrap_or(1))
}

fn build_passthrough_invocation(options: CodexWrapperOptions) -> anyhow::Result<CodexInvocation> {
    let mut args = Vec::new();
    if options.no_stateful {
        push_config(&mut args, "features.hooks", "false");
    }
    args.extend(options.args);

    Ok(CodexInvocation {
        program: options.codex_bin,
        args,
        env: vec![(
            STATEFUL_CODEX_RUN_ID_ENV.to_string(),
            uuid::Uuid::new_v4().to_string(),
        )],
    })
}

fn build_read_only_tmp_invocation(options: CodexWrapperOptions) -> anyhow::Result<CodexInvocation> {
    let writable_roots = tmp_writable_roots();
    let sandbox = serde_json::json!({
        "mode": "read-only",
        "writable_roots": writable_roots,
        "network_access": false,
        "source": "stateful-codex-wrapper",
        "profile": READ_ONLY_TMP_PROFILE,
    });

    let mut args = Vec::new();
    push_config(
        &mut args,
        "default_permissions",
        &toml_string(READ_ONLY_TMP_PROFILE)?,
    );
    push_config(
        &mut args,
        &format!("permissions.{READ_ONLY_TMP_PROFILE}.extends"),
        &toml_string(":read-only")?,
    );
    push_config(
        &mut args,
        &format!("permissions.{READ_ONLY_TMP_PROFILE}.network.enabled"),
        "false",
    );
    if options.no_stateful {
        push_config(&mut args, "features.hooks", "false");
    }
    push_config(&mut args, "web_search", &toml_string("live")?);
    push_config(
        &mut args,
        "mcp_servers.stateful.default_tools_approval_mode",
        &toml_string("approve")?,
    );
    for root in sandbox_writable_roots(&sandbox) {
        push_config(
            &mut args,
            &format!("permissions.{READ_ONLY_TMP_PROFILE}.filesystem.{root}"),
            &toml_string("write")?,
        );
    }
    args.extend(options.args);

    Ok(CodexInvocation {
        program: options.codex_bin,
        args,
        env: vec![(
            STATEFUL_CODEX_RUN_ID_ENV.to_string(),
            uuid::Uuid::new_v4().to_string(),
        )],
    })
}

fn push_config(args: &mut Vec<String>, key: &str, value: &str) {
    args.push("-c".to_string());
    args.push(format!("{key}={value}"));
}

fn sandbox_writable_roots(sandbox: &serde_json::Value) -> Vec<String> {
    sandbox["writable_roots"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn tmp_writable_roots() -> Vec<String> {
    let mut roots = BTreeSet::new();
    roots.insert("/tmp".to_string());
    roots.insert("/var/tmp".to_string());
    if cfg!(target_os = "macos") {
        roots.insert("/private/tmp".to_string());
    }
    if let Ok(tmpdir) = std::env::var("TMPDIR") {
        insert_non_empty_path(&mut roots, tmpdir);
    }
    insert_non_empty_path(&mut roots, path_to_string(std::env::temp_dir()));
    roots.into_iter().collect()
}

fn insert_non_empty_path(roots: &mut BTreeSet<String>, value: String) {
    let value = value.trim().trim_end_matches('/').to_string();
    if !value.is_empty() {
        roots.insert(value);
    }
}

fn path_to_string(path: PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

fn toml_string(value: &str) -> anyhow::Result<String> {
    Ok(serde_json::to_string(value)?)
}
