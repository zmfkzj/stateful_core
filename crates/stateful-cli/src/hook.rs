use std::{
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;
use serde_json::json;

use crate::outbox::queue_session_heartbeat_outbox;
use crate::{
    CurrentSession, GlobalPaths, HookCommand, ProtocolEnvelopeArgs, RepoGate, RepoIdentity,
    ServerRuntime, discover_runtime_with_global, ensure_server, get_json, post_json,
    protocol_envelope, repo_gate, repo_identity_for_enabled_repo,
    runtime_env_override_is_configured, write_current_session_file_for_codex_session,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    Allow,
    Deny { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SandboxRunInvocation {
    executable: String,
    fs: String,
    network: String,
    write_targets: Vec<String>,
    create_targets: Vec<String>,
    write_dirs: Vec<String>,
    command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NestedCodexBenchmarkSandboxInvocation {
    executable: String,
    purpose: String,
    write_dir: String,
    codex_home_root: String,
    command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatefulExternalRunInvocation {
    executable: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatefulControlInvocation {
    executable: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuoteState {
    None,
    Single,
    Double,
}

impl HookOutcome {
    pub fn to_stdout_json(&self) -> serde_json::Result<serde_json::Value> {
        match self {
            Self::Allow => Ok(json!({})),
            Self::Deny { reason } => Ok(json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": reason,
                }
            })),
        }
    }
}

pub fn run_hook(command: HookCommand) -> anyhow::Result<()> {
    match command {
        HookCommand::SessionStart => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            if let Err(error) = handle_session_start_in_repo(&input, hook_start_dir(&input)?) {
                eprintln!("stateful session-start warning: {error}");
            }
        }
        HookCommand::PostToolUse => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            if let Err(error) = handle_post_tool_use_in_repo(&input, hook_start_dir(&input)?) {
                eprintln!("stateful post-tool-use warning: {error}");
            }
        }
        HookCommand::UserPromptSubmit => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            match handle_user_prompt_submit_in_repo(&input, hook_start_dir(&input)?) {
                Ok(prompt_text) if !prompt_text.is_empty() => println!("{prompt_text}"),
                Ok(_) => {}
                Err(error) => eprintln!("stateful user-prompt-submit warning: {error}"),
            }
        }
        HookCommand::Stop => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            if let Err(error) = handle_stop_in_repo(&input, hook_start_dir(&input)?) {
                eprintln!("stateful stop warning: {error}");
            }
        }
        HookCommand::PreToolUse => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            let outcome = handle_pre_tool_use_in_repo(&input, hook_start_dir(&input)?)?;
            if !matches!(outcome, HookOutcome::Allow) {
                println!("{}", serde_json::to_string(&outcome.to_stdout_json()?)?);
            }
        }
    }

    Ok(())
}

pub fn handle_pre_tool_use(input: &str) -> anyhow::Result<HookOutcome> {
    handle_pre_tool_use_with_runtime(input, None, None, None)
}

pub fn handle_pre_tool_use_in_repo(
    input: &str,
    repo_root: impl AsRef<Path>,
) -> anyhow::Result<HookOutcome> {
    let start = hook_start_dir_or(input, repo_root.as_ref());
    let paths = GlobalPaths::from_env()?;
    let repo_root = match repo_gate(&paths, &start)? {
        RepoGate::Enabled { repo_root } => {
            if !runtime_env_override_is_configured() {
                ensure_server(&paths)?;
            }
            repo_root
        }
        RepoGate::Disabled | RepoGate::OutsideGitRepo => return Ok(HookOutcome::Allow),
    };
    let runtime = discover_runtime_with_global(&repo_root, &paths)?;
    remember_current_session(&repo_root, &runtime, input)?;
    handle_pre_tool_use_with_runtime(
        input,
        Some(&runtime),
        Some(&repo_root),
        Some(start.as_path()),
    )
}

pub fn handle_session_start_in_repo(
    input: &str,
    repo_root: impl AsRef<Path>,
) -> anyhow::Result<()> {
    let start = hook_start_dir_or(input, repo_root.as_ref());
    let paths = GlobalPaths::from_env()?;
    let repo_root = match repo_gate(&paths, start)? {
        RepoGate::Enabled { repo_root } => {
            if !runtime_env_override_is_configured() {
                ensure_server(&paths)?;
            }
            repo_root
        }
        RepoGate::Disabled | RepoGate::OutsideGitRepo => return Ok(()),
    };
    let runtime = discover_runtime_with_global(&repo_root, &paths)?;
    remember_current_session(&repo_root, &runtime, input)?;
    let identity = repo_identity(&paths, &repo_root)?;
    handle_session_start_with_runtime(input, &runtime, Some(&identity))
}

pub fn handle_post_tool_use_in_repo(
    input: &str,
    repo_root: impl AsRef<Path>,
) -> anyhow::Result<()> {
    let start = hook_start_dir_or(input, repo_root.as_ref());
    let paths = GlobalPaths::from_env()?;
    let repo_root = match repo_gate(&paths, start)? {
        RepoGate::Enabled { repo_root } => {
            if !runtime_env_override_is_configured() {
                ensure_server(&paths)?;
            }
            repo_root
        }
        RepoGate::Disabled | RepoGate::OutsideGitRepo => return Ok(()),
    };
    let runtime = discover_runtime_with_global(&repo_root, &paths)?;
    remember_current_session(&repo_root, &runtime, input)?;
    let identity = repo_identity(&paths, &repo_root)?;
    if let Err(error) = handle_post_tool_use_with_runtime(input, &runtime, Some(&identity)) {
        let input: SessionEventInput = serde_json::from_str(input)?;
        queue_session_heartbeat_outbox(
            &repo_root,
            &runtime.workspace_id,
            input.stateful_session_id(),
            &error.to_string(),
        )?;
    }

    Ok(())
}

pub fn handle_user_prompt_submit_in_repo(
    input: &str,
    repo_root: impl AsRef<Path>,
) -> anyhow::Result<String> {
    let start = hook_start_dir_or(input, repo_root.as_ref());
    let paths = GlobalPaths::from_env()?;
    let repo_root = match repo_gate(&paths, start)? {
        RepoGate::Enabled { repo_root } => {
            if !runtime_env_override_is_configured() {
                ensure_server(&paths)?;
            }
            repo_root
        }
        RepoGate::Disabled | RepoGate::OutsideGitRepo => return Ok(String::new()),
    };
    let runtime = discover_runtime_with_global(&repo_root, &paths)?;
    remember_current_session(&repo_root, &runtime, input)?;
    let identity = repo_identity(&paths, &repo_root)?;
    handle_user_prompt_submit_with_runtime(input, &runtime, Some(&identity))
}

pub fn handle_stop_in_repo(input: &str, repo_root: impl AsRef<Path>) -> anyhow::Result<()> {
    let start = hook_start_dir_or(input, repo_root.as_ref());
    let paths = GlobalPaths::from_env()?;
    let repo_root = match repo_gate(&paths, start)? {
        RepoGate::Enabled { repo_root } => {
            if !runtime_env_override_is_configured() {
                ensure_server(&paths)?;
            }
            repo_root
        }
        RepoGate::Disabled | RepoGate::OutsideGitRepo => return Ok(()),
    };
    let runtime = discover_runtime_with_global(&repo_root, &paths)?;
    remember_current_session(&repo_root, &runtime, input)?;
    let identity = repo_identity(&paths, &repo_root)?;
    handle_stop_with_runtime(input, &runtime, Some(&identity))
}

fn remember_current_session(
    repo_root: &Path,
    runtime: &ServerRuntime,
    input: &str,
) -> anyhow::Result<()> {
    let input: SessionEventInput = serde_json::from_str(input)?;
    write_current_session_file_for_codex_session(
        repo_root,
        &CurrentSession::new(input.stateful_session_id(), runtime.workspace_id.clone()),
    )
}

fn handle_session_start_with_runtime(
    input: &str,
    runtime: &ServerRuntime,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let input: SessionStartInput = serde_json::from_str(input)?;
    post_session_event(
        runtime,
        "/v1/session/register",
        input.stateful_session_id(),
        identity,
    )
}

fn handle_post_tool_use_with_runtime(
    input: &str,
    runtime: &ServerRuntime,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let input: SessionEventInput = serde_json::from_str(input)?;
    post_session_event(
        runtime,
        "/v1/session/heartbeat",
        input.stateful_session_id(),
        identity,
    )
}

fn handle_user_prompt_submit_with_runtime(
    input: &str,
    runtime: &ServerRuntime,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<String> {
    let input: UserPromptSubmitInput = serde_json::from_str(input)?;
    let mut body = json!({
        "session_id": input.stateful_session_id(),
        "workspace_id": runtime.workspace_id,
        "mode": "brief"
    });
    if let Some(identity) = identity
        && let Some(object) = body.as_object_mut()
    {
        object.insert("repo_id".to_string(), json!(&identity.repo_id));
        object.insert("worktree_id".to_string(), json!(&identity.worktree_id));
        object.insert("root".to_string(), json!(&identity.root));
        object.insert("branch".to_string(), json!(&identity.branch));
    }
    let response = post_json(runtime, "/v1/context/render", &body)?;

    if !(200..300).contains(&response.status_code) {
        anyhow::bail!(
            "context render failed with HTTP {}: {}",
            response.status_code,
            response.body
        );
    }

    let response: ContextRenderResponse = serde_json::from_str(&response.body)?;
    Ok(with_stateful_command_policy_reminder(response.prompt_text))
}

fn with_stateful_command_policy_reminder(prompt_text: String) -> String {
    let reminder = stateful_command_policy_reminder();
    if prompt_text.trim().is_empty() {
        reminder
    } else {
        format!("{reminder}\n\n{prompt_text}")
    }
}

fn stateful_command_policy_reminder() -> String {
    let binary = trusted_stateful_binary_for_guidance();
    format!(
        "Stateful command policy reminder:\n- Before using Bash, use the `stateful-command-policy` skill.\n- Raw Bash is denied; use `{binary} sandbox run --fs read-only --network disabled --command '<cmd>'` for shell inspection.\n- For file edits, declare exact intent, acquire the same-session file lease successfully, then use native Codex edit tools such as `apply_patch` or Edit.\n- For command-shaped writes, declare exact intent, acquire the matching file or directory lease successfully, then use `{binary} sandbox run --fs write-targets --write-target <file> --command '<cmd>'`, `{binary} sandbox run --fs write-targets --create-target <file> --command '<cmd>'`, or `{binary} sandbox run --fs write-targets --write-dir target --command '<cmd>'` for target/ artifacts."
    )
}

fn handle_stop_with_runtime(
    input: &str,
    runtime: &ServerRuntime,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let input: SessionEventInput = serde_json::from_str(input)?;
    post_session_event(
        runtime,
        "/v1/activity/finalize",
        input.stateful_session_id(),
        identity,
    )
}

fn post_session_event(
    runtime: &ServerRuntime,
    path: &str,
    session_id: &str,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<()> {
    let mut body = json!({
        "session_id": session_id,
        "workspace_id": runtime.workspace_id,
    });
    if let Some(identity) = identity {
        body["repo_id"] = json!(&identity.repo_id);
        body["worktree_id"] = json!(&identity.worktree_id);
        body["root"] = json!(&identity.root);
        body["branch"] = json!(&identity.branch);
    }

    let response = post_json(runtime, path, &body)?;

    if !(200..300).contains(&response.status_code) {
        anyhow::bail!(
            "session event failed with HTTP {}: {}",
            response.status_code,
            response.body
        );
    }

    Ok(())
}

fn handle_pre_tool_use_with_runtime(
    input: &str,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
) -> anyhow::Result<HookOutcome> {
    let input: PreToolUseInput = serde_json::from_str(input)?;
    let identity = repo_root.and_then(|repo_root| {
        GlobalPaths::from_env()
            .ok()
            .and_then(|paths| repo_identity_for_enabled_repo(&paths, repo_root).ok())
    });

    match input.tool_name.as_str() {
        "Bash" => authorize_bash(&input),
        "apply_patch" => authorize_apply_patch(&input, runtime, repo_root, cwd, identity.as_ref()),
        "file_change" => {
            authorize_file_change_tool(&input, runtime, repo_root, cwd, identity.as_ref())
        }
        "Edit" | "Write" => {
            authorize_file_write_tool(&input, runtime, repo_root, cwd, identity.as_ref())
        }
        tool_name if tool_name.starts_with("mcp__filesystem__") => Ok(HookOutcome::Deny {
            reason: "filesystem MCP writes require stateful authorization; read-only MCP calls are not yet classified".to_string(),
        }),
        _ => Ok(HookOutcome::Allow),
    }
}

fn authorize_bash(input: &PreToolUseInput) -> anyhow::Result<HookOutcome> {
    let command = input.command().unwrap_or_default();
    let sandbox = authorize_sandbox_run_bash(command);
    if sandbox == HookOutcome::Allow {
        return Ok(sandbox);
    }

    let external = authorize_external_run_bash(command);
    if external == HookOutcome::Allow || command.contains("external-run") {
        return Ok(external);
    }

    let control = authorize_stateful_control_bash(command);
    if control == HookOutcome::Allow || command_mentions_stateful_control(command) {
        return Ok(control);
    }

    Ok(sandbox)
}

fn authorize_sandbox_run_bash(command: &str) -> HookOutcome {
    if split_simple_command_words(command)
        .ok()
        .is_some_and(|words| {
            words.len() >= 3 && words[1] == "sandbox" && words[2] == "run-nested-codex-benchmark"
        })
    {
        return authorize_nested_codex_benchmark_sandbox_bash(command);
    }

    let invocation = match parse_sandbox_run_invocation(command) {
        Ok(invocation) => invocation,
        Err(reason) => return bash_policy_deny(reason),
    };

    if !is_trusted_stateful_executable(&invocation.executable) {
        return bash_policy_deny(
            "stateful sandbox run requires the trusted absolute stateful binary",
        );
    }
    if !matches!(invocation.fs.as_str(), "read-only" | "write-targets") {
        return bash_policy_deny(
            "stateful sandbox run supports only read-only and write-targets profiles",
        );
    }
    if !matches!(invocation.network.as_str(), "disabled" | "enabled") {
        return bash_policy_deny("stateful sandbox run network must be disabled or enabled");
    }
    if invocation.command.trim().is_empty() {
        return bash_policy_deny("stateful sandbox run requires a non-empty --command");
    }
    if invocation.fs == "read-only"
        && (!invocation.write_targets.is_empty()
            || !invocation.create_targets.is_empty()
            || !invocation.write_dirs.is_empty())
    {
        return bash_policy_deny(
            "read-only sandbox run rejects write targets, create targets, and write dirs",
        );
    }
    if invocation.fs == "write-targets"
        && invocation.write_targets.is_empty()
        && invocation.create_targets.is_empty()
        && invocation.write_dirs.is_empty()
    {
        return bash_policy_deny(
            "write-targets sandbox run requires at least one write target, create target, or write dir",
        );
    }

    HookOutcome::Allow
}

fn authorize_nested_codex_benchmark_sandbox_bash(command: &str) -> HookOutcome {
    let invocation = match parse_nested_codex_benchmark_sandbox_invocation(command) {
        Ok(invocation) => invocation,
        Err(reason) => return bash_policy_deny(reason),
    };

    if !is_trusted_stateful_executable(&invocation.executable) {
        return bash_policy_deny(
            "stateful sandbox run-nested-codex-benchmark requires the trusted absolute stateful binary",
        );
    }
    if invocation.purpose.trim().is_empty() {
        return bash_policy_deny("stateful sandbox run-nested-codex-benchmark requires --purpose");
    }
    if invocation.write_dir != "target" {
        return bash_policy_deny(
            "stateful sandbox run-nested-codex-benchmark requires --write-dir target",
        );
    }
    if !hook_path_is_under_target(&invocation.codex_home_root) {
        return bash_policy_deny(
            "stateful sandbox run-nested-codex-benchmark requires --codex-home-root under target",
        );
    }
    if invocation.command.trim().is_empty() {
        return bash_policy_deny(
            "stateful sandbox run-nested-codex-benchmark requires a non-empty --command",
        );
    }

    HookOutcome::Allow
}

fn authorize_external_run_bash(command: &str) -> HookOutcome {
    let invocation = match parse_external_run_invocation(command) {
        Ok(invocation) => invocation,
        Err(reason) => return bash_policy_deny(reason),
    };

    if !is_trusted_stateful_executable(&invocation.executable) {
        return bash_policy_deny(
            "stateful external-run requires the trusted absolute stateful binary",
        );
    }

    HookOutcome::Allow
}

fn authorize_stateful_control_bash(command: &str) -> HookOutcome {
    let invocation = match parse_stateful_control_invocation(command) {
        Ok(invocation) => invocation,
        Err(reason) => return bash_policy_deny(reason),
    };

    if !is_trusted_stateful_executable(&invocation.executable) {
        return bash_policy_deny(
            "stateful control commands require the trusted absolute stateful binary",
        );
    }

    HookOutcome::Allow
}

fn bash_policy_deny(reason: impl Into<String>) -> HookOutcome {
    HookOutcome::Deny {
        reason: format!("{} {}", reason.into(), bash_policy_guidance()),
    }
}

fn bash_policy_guidance() -> String {
    format!(
        "Use the `stateful-command-policy` skill before Bash. Raw Bash is denied; for read-only shell inspection use `{} sandbox run --fs read-only --network disabled --command '<cmd>'`; for approved repo-external writes use `{} external-run request --purpose '<purpose>' --write-dir <dir> --command '<cmd>'`.",
        trusted_stateful_binary_for_guidance(),
        trusted_stateful_binary_for_guidance()
    )
}

fn trusted_stateful_binary_for_guidance() -> String {
    std::env::current_exe()
        .ok()
        .map(|path| format!("\"{}\"", path.to_string_lossy()))
        .unwrap_or_else(|| "<absolute-stateful-binary>".to_string())
}

fn parse_sandbox_run_invocation(command: &str) -> Result<SandboxRunInvocation, String> {
    reject_outer_shell_syntax(
        command,
        "Bash wrapper must be a single stateful sandbox run command",
    )?;
    let words = split_simple_command_words(command)?;
    if words.is_empty() {
        return Err("Bash commands must use stateful sandbox run".to_string());
    }
    if first_word_is_env_assignment(&words[0]) {
        return Err("Bash wrapper must not use outer environment assignments".to_string());
    }
    if words.len() < 3 || words[1] != "sandbox" || words[2] != "run" {
        return Err("Bash commands must use stateful sandbox run".to_string());
    }

    let mut fs = "read-only".to_string();
    let mut network = "disabled".to_string();
    let mut write_targets = Vec::new();
    let mut create_targets = Vec::new();
    let mut write_dirs = Vec::new();
    let mut inner_command = None;
    let mut index = 3;
    while index < words.len() {
        let arg = &words[index];
        match arg.as_str() {
            "--" => {
                return Err("stateful sandbox run does not support argv mode".to_string());
            }
            "--fs" => {
                index += 1;
                fs = parse_sandbox_run_arg_value(&words, index, "--fs")?;
            }
            "--network" => {
                index += 1;
                network = parse_sandbox_run_arg_value(&words, index, "--network")?;
            }
            "--write-target" => {
                index += 1;
                write_targets.push(parse_sandbox_run_arg_value(
                    &words,
                    index,
                    "--write-target",
                )?);
            }
            "--create-target" => {
                index += 1;
                create_targets.push(parse_sandbox_run_arg_value(
                    &words,
                    index,
                    "--create-target",
                )?);
            }
            "--write-dir" => {
                index += 1;
                write_dirs.push(parse_sandbox_run_arg_value(&words, index, "--write-dir")?);
            }
            "--command" => {
                if inner_command.is_some() {
                    return Err("stateful sandbox run requires exactly one --command".to_string());
                }
                index += 1;
                inner_command = Some(parse_sandbox_run_arg_value(&words, index, "--command")?);
            }
            "--timeout-seconds" => {
                index += 1;
                let timeout = parse_sandbox_run_arg_value(&words, index, "--timeout-seconds")?;
                if timeout.parse::<u64>().is_err() {
                    return Err(
                        "stateful sandbox run --timeout-seconds requires an integer value"
                            .to_string(),
                    );
                }
            }
            _ => {
                return Err(format!("unsupported stateful sandbox run argument `{arg}`"));
            }
        }
        index += 1;
    }

    let Some(command) = inner_command else {
        return Err("stateful sandbox run requires exactly one --command".to_string());
    };

    Ok(SandboxRunInvocation {
        executable: words[0].clone(),
        fs,
        network,
        write_targets,
        create_targets,
        write_dirs,
        command,
    })
}

fn parse_nested_codex_benchmark_sandbox_invocation(
    command: &str,
) -> Result<NestedCodexBenchmarkSandboxInvocation, String> {
    reject_outer_shell_syntax(
        command,
        "Bash wrapper must be a single stateful sandbox run-nested-codex-benchmark command",
    )?;
    let words = split_simple_command_words(command)?;
    if words.is_empty() {
        return Err("Bash commands must use stateful sandbox run".to_string());
    }
    if first_word_is_env_assignment(&words[0]) {
        return Err("Bash wrapper must not use outer environment assignments".to_string());
    }
    if words.len() < 3 || words[1] != "sandbox" || words[2] != "run-nested-codex-benchmark" {
        return Err("Bash commands must use stateful sandbox run".to_string());
    }

    let mut purpose = None;
    let mut write_dir = None;
    let mut codex_home_root = None;
    let mut inner_command = None;
    let mut index = 3;
    while index < words.len() {
        let arg = &words[index];
        match arg.as_str() {
            "--" => {
                return Err(
                    "stateful sandbox run-nested-codex-benchmark does not support argv mode"
                        .to_string(),
                );
            }
            "--purpose" => {
                index += 1;
                purpose = Some(parse_sandbox_run_arg_value(&words, index, "--purpose")?);
            }
            "--write-dir" => {
                index += 1;
                write_dir = Some(parse_sandbox_run_arg_value(&words, index, "--write-dir")?);
            }
            "--codex-home-root" => {
                index += 1;
                codex_home_root = Some(parse_sandbox_run_arg_value(
                    &words,
                    index,
                    "--codex-home-root",
                )?);
            }
            "--command" => {
                if inner_command.is_some() {
                    return Err(
                        "stateful sandbox run-nested-codex-benchmark requires exactly one --command"
                            .to_string(),
                    );
                }
                index += 1;
                inner_command = Some(parse_sandbox_run_arg_value(&words, index, "--command")?);
            }
            "--timeout-seconds" => {
                index += 1;
                let timeout = parse_sandbox_run_arg_value(&words, index, "--timeout-seconds")?;
                if timeout.parse::<u64>().is_err() {
                    return Err(
                        "stateful sandbox run-nested-codex-benchmark --timeout-seconds requires an integer value"
                            .to_string(),
                    );
                }
            }
            _ => {
                return Err(format!(
                    "unsupported stateful sandbox run-nested-codex-benchmark argument `{arg}`"
                ));
            }
        }
        index += 1;
    }

    let Some(purpose) = purpose else {
        return Err("stateful sandbox run-nested-codex-benchmark requires --purpose".to_string());
    };
    let Some(write_dir) = write_dir else {
        return Err(
            "stateful sandbox run-nested-codex-benchmark requires --write-dir target".to_string(),
        );
    };
    let Some(codex_home_root) = codex_home_root else {
        return Err(
            "stateful sandbox run-nested-codex-benchmark requires --codex-home-root".to_string(),
        );
    };
    let Some(command) = inner_command else {
        return Err(
            "stateful sandbox run-nested-codex-benchmark requires exactly one --command"
                .to_string(),
        );
    };

    Ok(NestedCodexBenchmarkSandboxInvocation {
        executable: words[0].clone(),
        purpose,
        write_dir,
        codex_home_root,
        command,
    })
}

fn parse_external_run_invocation(command: &str) -> Result<StatefulExternalRunInvocation, String> {
    reject_outer_shell_syntax(command, "Bash wrapper must be a single stateful command")?;
    let words = split_simple_command_words(command)?;
    if words.is_empty() {
        return Err(
            "Bash commands must use stateful sandbox run or stateful external-run".to_string(),
        );
    }
    if first_word_is_env_assignment(&words[0]) {
        return Err("Bash wrapper must not use outer environment assignments".to_string());
    }
    if words.len() < 3 || words[1] != "external-run" {
        return Err(
            "Bash commands must use stateful sandbox run or stateful external-run".to_string(),
        );
    }
    if !matches!(words[2].as_str(), "request" | "approve" | "run" | "help") {
        return Err("stateful external-run requires request, approve, or run".to_string());
    }

    Ok(StatefulExternalRunInvocation {
        executable: words[0].clone(),
    })
}

fn parse_stateful_control_invocation(command: &str) -> Result<StatefulControlInvocation, String> {
    reject_outer_shell_syntax(command, "Bash wrapper must be a single stateful command")?;
    let words = split_simple_command_words(command)?;
    if words.is_empty() {
        return Err("Bash commands must use a supported stateful command".to_string());
    }
    if first_word_is_env_assignment(&words[0]) {
        return Err("Bash wrapper must not use outer environment assignments".to_string());
    }
    if words.len() < 2 || !is_stateful_control_command(&words[1]) {
        return Err(
            "Bash commands must use stateful sandbox run, stateful external-run, or a trusted stateful commit, push, or server command"
                .to_string(),
        );
    }

    Ok(StatefulControlInvocation {
        executable: words[0].clone(),
    })
}

fn command_mentions_stateful_control(command: &str) -> bool {
    split_simple_command_words(command)
        .ok()
        .and_then(|words| words.get(1).cloned())
        .is_some_and(|command| is_stateful_control_command(&command))
}

fn is_stateful_control_command(command: &str) -> bool {
    matches!(command, "commit" | "push" | "server")
}

fn parse_sandbox_run_arg_value(
    words: &[String],
    index: usize,
    arg: &str,
) -> Result<String, String> {
    words
        .get(index)
        .cloned()
        .ok_or_else(|| format!("stateful sandbox run argument `{arg}` requires a value"))
}

fn reject_outer_shell_syntax(command: &str, single_command_message: &str) -> Result<(), String> {
    let mut state = QuoteState::None;
    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
        match state {
            QuoteState::None => match ch {
                '\'' => state = QuoteState::Single,
                '"' => state = QuoteState::Double,
                '$' if chars.peek().is_some_and(|next| *next == '(') => {
                    return Err("Bash wrapper must not use command substitution".to_string());
                }
                '\\' => {
                    return Err("Bash wrapper must not use shell escapes".to_string());
                }
                ';' | '|' | '&' | '<' | '>' | '\n' | '\r' | '`' => {
                    return Err(single_command_message.to_string());
                }
                _ => {}
            },
            QuoteState::Single => {
                if ch == '\'' {
                    state = QuoteState::None;
                }
            }
            QuoteState::Double => match ch {
                '"' => state = QuoteState::None,
                '$' if chars.peek().is_some_and(|next| *next == '(') => {
                    return Err("Bash wrapper must not use command substitution".to_string());
                }
                '`' => {
                    return Err("Bash wrapper must not use command substitution".to_string());
                }
                '\\' => {
                    return Err("Bash wrapper must not use shell escapes".to_string());
                }
                _ => {}
            },
        }
    }

    if state != QuoteState::None {
        return Err("Bash wrapper command has unterminated quotes".to_string());
    }

    Ok(())
}

fn split_simple_command_words(command: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut state = QuoteState::None;
    let mut in_word = false;

    for ch in command.chars() {
        match state {
            QuoteState::None => match ch {
                '\'' => {
                    state = QuoteState::Single;
                    in_word = true;
                }
                '"' => {
                    state = QuoteState::Double;
                    in_word = true;
                }
                ch if ch.is_whitespace() => {
                    if in_word {
                        words.push(std::mem::take(&mut current));
                        in_word = false;
                    }
                }
                _ => {
                    current.push(ch);
                    in_word = true;
                }
            },
            QuoteState::Single => {
                if ch == '\'' {
                    state = QuoteState::None;
                } else {
                    current.push(ch);
                }
            }
            QuoteState::Double => {
                if ch == '"' {
                    state = QuoteState::None;
                } else {
                    current.push(ch);
                }
            }
        }
    }

    if state != QuoteState::None {
        return Err("Bash wrapper command has unterminated quotes".to_string());
    }
    if in_word {
        words.push(current);
    }

    Ok(words)
}

fn first_word_is_env_assignment(word: &str) -> bool {
    let Some((name, _value)) = word.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
        && !name.chars().next().is_some_and(|c| c.is_ascii_digit())
}

fn hook_path_is_under_target(path: &str) -> bool {
    let trimmed = path.trim().replace('\\', "/");
    if trimmed.is_empty() || trimmed.starts_with('/') {
        return false;
    }
    let mut segments = Vec::new();
    for segment in trimmed.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." || segment.eq_ignore_ascii_case(".git") {
            return false;
        }
        if segment.chars().any(char::is_control) {
            return false;
        }
        segments.push(segment);
    }
    segments.len() >= 2 && segments[0] == "target"
}

fn is_trusted_stateful_executable(executable: &str) -> bool {
    let path = Path::new(executable);
    if !path.is_absolute() {
        return false;
    }
    let Ok(candidate) = path.canonicalize() else {
        return false;
    };
    let Ok(current) = std::env::current_exe() else {
        return false;
    };
    let current = current.canonicalize().unwrap_or(current);
    candidate == current
}

fn authorize_apply_patch(
    input: &PreToolUseInput,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<HookOutcome> {
    let Some(patch) = input.patch_text() else {
        return Ok(HookOutcome::Deny {
            reason:
                "apply_patch writes require patch text with file targets for stateful authorization"
                    .to_string(),
        });
    };
    let targets = extract_apply_patch_write_targets(patch);
    if targets.is_empty() {
        return Ok(HookOutcome::Deny {
            reason:
                "apply_patch writes require at least one file target for stateful authorization"
                    .to_string(),
        });
    }
    let Some(targets) = normalize_targets(targets, repo_root, cwd)? else {
        return Ok(HookOutcome::Deny {
            reason: "apply_patch target is outside the enabled repo".to_string(),
        });
    };
    authorize_targets(input, runtime, targets, identity)
}

fn authorize_file_change_tool(
    input: &PreToolUseInput,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<HookOutcome> {
    let targets = extract_file_change_targets(&input.tool_input);
    if targets.is_empty() {
        return Ok(HookOutcome::Deny {
            reason: "file_change writes require changed file targets for stateful authorization"
                .to_string(),
        });
    }
    let Some(targets) = normalize_targets(targets, repo_root, cwd)? else {
        return Ok(HookOutcome::Deny {
            reason: "file_change target is outside the enabled repo".to_string(),
        });
    };
    authorize_targets(input, runtime, targets, identity)
}

fn authorize_file_write_tool(
    input: &PreToolUseInput,
    runtime: Option<&ServerRuntime>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<HookOutcome> {
    let Some(path) = input
        .tool_input
        .get("file_path")
        .or_else(|| input.tool_input.get("path"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
    else {
        return Ok(HookOutcome::Deny {
            reason: format!(
                "{} writes require a file_path target for stateful authorization",
                input.tool_name
            ),
        });
    };

    let Some(target) = normalize_file_tool_target(path, repo_root, cwd)? else {
        return Ok(HookOutcome::Deny {
            reason: format!("{} target is outside the enabled repo", input.tool_name),
        });
    };

    authorize_targets(input, runtime, vec![PatchTarget::write(&target)], identity)
}

fn normalize_targets(
    targets: Vec<PatchTarget>,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
) -> anyhow::Result<Option<Vec<PatchTarget>>> {
    let mut normalized = Vec::with_capacity(targets.len());
    for target in targets {
        let Some(path) = normalize_file_tool_target(&target.path, repo_root, cwd)? else {
            return Ok(None);
        };
        let new_path = if let Some(new_path) = &target.new_path {
            let Some(new_path) = normalize_file_tool_target(new_path, repo_root, cwd)? else {
                return Ok(None);
            };
            Some(new_path)
        } else {
            None
        };
        normalized.push(target.with_paths(path, new_path));
    }

    Ok(Some(normalized))
}

fn percent_encode_current_resource(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            65..=90 | 97..=122 | 48..=57 | 45 | 46 | 47 | 95 | 126 => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push(37 as char);
                encoded.push(HEX[(byte >> 4) as usize] as char);
                encoded.push(HEX[(byte & 0x0f) as usize] as char);
            }
        }
    }
    encoded
}

fn hook_authorize_purpose(
    input: &PreToolUseInput,
    runtime: &ServerRuntime,
    target: &PatchTarget,
) -> Option<String> {
    let resource = percent_encode_current_resource(&target.path);
    let response = get_json(runtime, &format!("/v1/current?resource={resource}")).ok()?;
    if !(200..300).contains(&response.status_code) {
        return None;
    }
    let body: serde_json::Value = serde_json::from_str(&response.body).ok()?;
    let items = body.get("items")?.as_array()?;
    items.iter().find_map(|item| {
        let matches_intent = item.get("kind").and_then(serde_json::Value::as_str) == Some("intent")
            && item.get("freshness").and_then(serde_json::Value::as_str) == Some("live")
            && item.get("resource").and_then(serde_json::Value::as_str)
                == Some(target.path.as_str())
            && item.get("session_id").and_then(serde_json::Value::as_str)
                == Some(input.stateful_session_id())
            && item.get("workspace_id").and_then(serde_json::Value::as_str)
                == Some(runtime.workspace_id.as_str());
        if !matches_intent {
            return None;
        }
        item.get("purpose")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|purpose| !purpose.is_empty())
            .map(str::to_string)
    })
}

fn authorize_targets(
    input: &PreToolUseInput,
    runtime: Option<&ServerRuntime>,
    targets: Vec<PatchTarget>,
    identity: Option<&RepoIdentity>,
) -> anyhow::Result<HookOutcome> {
    let Some(runtime) = runtime else {
        return Ok(HookOutcome::Deny {
            reason: format!(
                "{} writes require a reachable stateful server, exact file intent, and a same-session file lease",
                input.tool_name
            ),
        });
    };

    if targets.is_empty() {
        return Ok(HookOutcome::Allow);
    }

    for target in targets {
        let purpose = hook_authorize_purpose(input, runtime, &target);
        let mut payload = json!({
            "action": target.action,
            "path": target.path,
        });
        if let Some(purpose) = purpose {
            payload["queue_on_conflict"] = json!(true);
            payload["purpose"] = json!(purpose);
        }
        if let Some(new_path) = &target.new_path {
            payload["old_path"] = json!(target.path);
            payload["new_path"] = json!(new_path);
        }
        let body = protocol_envelope(ProtocolEnvelopeArgs {
            runtime,
            request_id: uuid::Uuid::new_v4().to_string(),
            session_id: input.stateful_session_id().to_string(),
            workspace_id: runtime.workspace_id.clone(),
            identity: identity.cloned(),
            source_kind: "hook",
            event: "pre_tool_use",
            source_ref: "hook:pre_tool_use",
            source_tool_name: Some(input.tool_name.as_str()),
            payload,
        });
        let response = post_json(runtime, "/v1/authorize", &body)?;

        if !(200..300).contains(&response.status_code) {
            return Ok(HookOutcome::Deny {
                reason: format!(
                    "stateful authorization failed with HTTP {}: {}",
                    response.status_code, response.body
                ),
            });
        }

        let decision: AuthorizeDecision = serde_json::from_str(&response.body)?;
        if decision.decision != "allow" {
            return Ok(HookOutcome::Deny {
                reason: decision.required_next_action.unwrap_or(decision.message),
            });
        }
    }

    Ok(HookOutcome::Allow)
}

fn normalize_file_tool_target(
    path: &str,
    repo_root: Option<&Path>,
    cwd: Option<&Path>,
) -> anyhow::Result<Option<String>> {
    let path = path.trim();
    let candidate = Path::new(path);
    let Some(repo_root) = repo_root else {
        return Ok(Some(path.replace('\\', "/")));
    };
    let repo_root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());

    if candidate.is_absolute() {
        let canonical = candidate
            .canonicalize()
            .unwrap_or_else(|_| candidate.to_path_buf());
        let candidate = normalize_path(canonical);
        let Ok(relative) = candidate.strip_prefix(&repo_root) else {
            return Ok(None);
        };
        return Ok(Some(relative.to_string_lossy().replace('\\', "/")));
    }

    let base = cwd.unwrap_or(repo_root.as_path());
    let base = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
    let candidate = normalize_path(base.join(candidate));
    let Ok(relative) = candidate.strip_prefix(&repo_root) else {
        return Ok(None);
    };

    Ok(Some(relative.to_string_lossy().replace('\\', "/")))
}

fn extract_apply_patch_write_targets(patch: &str) -> Vec<PatchTarget> {
    let mut targets = Vec::new();
    let mut pending_update: Option<String> = None;

    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("*** Update File: ") {
            if let Some(path) = pending_update.replace(path.trim().to_string())
                && !path.is_empty()
            {
                targets.push(PatchTarget::write(&path));
            }
        } else if let Some(path) = line.strip_prefix("*** Move to: ") {
            if let Some(old_path) = pending_update.take() {
                let new_path = path.trim();
                if !old_path.is_empty() && !new_path.is_empty() {
                    targets.push(PatchTarget::move_file(&old_path, new_path));
                }
            }
        } else if let Some(path) = line.strip_prefix("*** Add File: ") {
            if let Some(path) = pending_update.take()
                && !path.is_empty()
            {
                targets.push(PatchTarget::write(&path));
            }
            targets.push(PatchTarget::write(path));
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            if let Some(path) = pending_update.take()
                && !path.is_empty()
            {
                targets.push(PatchTarget::write(&path));
            }
            targets.push(PatchTarget::delete(path));
        }
    }

    if let Some(path) = pending_update
        && !path.is_empty()
    {
        targets.push(PatchTarget::write(&path));
    }

    targets
        .into_iter()
        .filter(|target| !target.path.is_empty())
        .collect()
}

fn extract_file_change_targets(input: &serde_json::Value) -> Vec<PatchTarget> {
    let Some(changes) = input.get("changes").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };

    changes
        .iter()
        .filter_map(|change| {
            let path = change
                .get("path")
                .and_then(serde_json::Value::as_str)?
                .trim();
            if path.is_empty() {
                return None;
            }
            let kind = change
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("update");
            if kind.eq_ignore_ascii_case("delete")
                || kind.eq_ignore_ascii_case("deleted")
                || kind.eq_ignore_ascii_case("remove")
                || kind.eq_ignore_ascii_case("removed")
            {
                Some(PatchTarget::delete(path))
            } else {
                Some(PatchTarget::write(path))
            }
        })
        .collect()
}

fn string_payload_field<'a>(value: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    if let Some(text) = value.as_str() {
        return Some(text);
    }

    for key in keys {
        if let Some(text) = value.get(key).and_then(serde_json::Value::as_str) {
            return Some(text);
        }
    }

    for nested_key in ["arguments", "args"] {
        if let Some(nested) = value.get(nested_key)
            && let Some(text) = string_payload_field(nested, keys)
        {
            return Some(text);
        }
    }

    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PatchTarget {
    action: &'static str,
    path: String,
    new_path: Option<String>,
}

impl PatchTarget {
    fn write(path: &str) -> Self {
        Self {
            action: "write_file",
            path: path.trim().to_string(),
            new_path: None,
        }
    }

    fn delete(path: &str) -> Self {
        Self {
            action: "delete_file",
            path: path.trim().to_string(),
            new_path: None,
        }
    }

    fn move_file(old_path: &str, new_path: &str) -> Self {
        Self {
            action: "move_file",
            path: old_path.trim().to_string(),
            new_path: Some(new_path.trim().to_string()),
        }
    }

    fn with_paths(mut self, path: String, new_path: Option<String>) -> Self {
        self.path = path;
        self.new_path = new_path;
        self
    }
}

fn repo_identity(paths: &GlobalPaths, repo_root: &Path) -> anyhow::Result<RepoIdentity> {
    repo_identity_for_enabled_repo(paths, repo_root)
}

fn hook_start_dir(input: &str) -> anyhow::Result<PathBuf> {
    let fallback = std::env::current_dir()?;
    Ok(hook_start_dir_or(input, &fallback))
}

fn hook_start_dir_or(input: &str, fallback: &Path) -> PathBuf {
    let Ok(input) = serde_json::from_str::<HookCwdInput>(input) else {
        return fallback.to_path_buf();
    };
    input
        .cwd
        .filter(|cwd| !cwd.as_os_str().is_empty() && cwd.exists())
        .unwrap_or_else(|| fallback.to_path_buf())
}

fn normalize_path(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(segment) => normalized.push(segment),
        }
    }
    normalized
}

#[derive(Debug, Deserialize)]
struct PreToolUseInput {
    session_id: String,
    #[serde(default)]
    thread_id: Option<String>,
    tool_name: String,
    #[serde(default)]
    tool_input: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct HookCwdInput {
    #[serde(default)]
    cwd: Option<PathBuf>,
}

impl PreToolUseInput {
    fn stateful_session_id(&self) -> &str {
        codex_thread_id_or_session_id(self.thread_id.as_deref(), &self.session_id)
    }

    fn command(&self) -> Option<&str> {
        self.tool_input.get("command")?.as_str()
    }

    fn patch_text(&self) -> Option<&str> {
        string_payload_field(
            &self.tool_input,
            &["command", "patch", "input", "cmd", "diff"],
        )
    }
}

#[derive(Debug, Deserialize)]
struct AuthorizeDecision {
    decision: String,
    message: String,
    #[serde(default)]
    required_next_action: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SessionStartInput {
    session_id: String,
    #[serde(default)]
    thread_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SessionEventInput {
    session_id: String,
    #[serde(default)]
    thread_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UserPromptSubmitInput {
    session_id: String,
    #[serde(default)]
    thread_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContextRenderResponse {
    prompt_text: String,
}

fn codex_thread_id_or_session_id<'a>(thread_id: Option<&'a str>, session_id: &'a str) -> &'a str {
    thread_id
        .filter(|thread_id| !thread_id.is_empty())
        .unwrap_or(session_id)
}

impl SessionStartInput {
    fn stateful_session_id(&self) -> &str {
        codex_thread_id_or_session_id(self.thread_id.as_deref(), &self.session_id)
    }
}

impl SessionEventInput {
    fn stateful_session_id(&self) -> &str {
        codex_thread_id_or_session_id(self.thread_id.as_deref(), &self.session_id)
    }
}

impl UserPromptSubmitInput {
    fn stateful_session_id(&self) -> &str {
        codex_thread_id_or_session_id(self.thread_id.as_deref(), &self.session_id)
    }
}
