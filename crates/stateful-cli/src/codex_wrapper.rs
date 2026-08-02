use std::process::{Command as ProcessCommand, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexWrapperOptions {
    pub codex_bin: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexInvocation {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub fn build_codex_invocation(options: CodexWrapperOptions) -> anyhow::Result<CodexInvocation> {
    Ok(CodexInvocation {
        program: options.codex_bin,
        args: options.args,
        env: Vec::new(),
    })
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
