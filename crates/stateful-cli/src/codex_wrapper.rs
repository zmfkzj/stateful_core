use std::process::{Command as ProcessCommand, Stdio};

use clap::ValueEnum;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CodexSandboxMode {
    Passthrough,
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
        env: Vec::new(),
    })
}

fn push_config(args: &mut Vec<String>, key: &str, value: &str) {
    args.push("-c".to_string());
    args.push(format!("{key}={value}"));
}
