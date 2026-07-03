use crate::shell_command::{
    first_word_is_env_assignment, reject_outer_shell_syntax, split_simple_command_words,
};

use super::{
    SandboxFsProfile, SandboxNetworkPolicy, SandboxProcessFindBashInvocation,
    SandboxProcessFindRequest, SandboxRunBashInvocation, SandboxRunRequest,
};

pub(crate) fn parse_sandbox_run_bash_invocation(
    command: &str,
) -> Result<SandboxRunBashInvocation, String> {
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

    let mut fs = SandboxFsProfile::ReadOnly;
    let mut network = SandboxNetworkPolicy::Disabled;
    let mut purpose = None;
    let mut reservation_id = None;
    let mut agent_id = None;
    let mut workspace_id = None;
    let mut write_targets = Vec::new();
    let mut create_targets = Vec::new();
    let mut write_dirs = Vec::new();
    let mut connect_sockets = Vec::new();
    let mut allow_signal = false;
    let mut inner_command = None;
    let mut sequences = Vec::new();
    let mut sequence_shell = None;
    let mut timeout_seconds = None;
    let mut stream_events = false;
    let mut index = 3;
    while index < words.len() {
        let arg = &words[index];
        match arg.as_str() {
            "--" => {
                return Err("stateful sandbox run does not support argv mode".to_string());
            }
            "--fs" => {
                index += 1;
                let value = parse_sandbox_run_arg_value(&words, index, "--fs")?;
                fs = parse_sandbox_fs_profile(&value)?;
            }
            "--network" => {
                index += 1;
                let value = parse_sandbox_run_arg_value(&words, index, "--network")?;
                network = parse_sandbox_network_policy(&value)?;
            }
            "--purpose" => {
                if purpose.is_some() {
                    return Err("stateful sandbox run accepts at most one --purpose".to_string());
                }
                index += 1;
                purpose = Some(parse_sandbox_run_arg_value(&words, index, "--purpose")?);
            }
            "--reservation-id" => {
                if reservation_id.is_some() {
                    return Err(
                        "stateful sandbox run accepts at most one --reservation-id".to_string()
                    );
                }
                index += 1;
                reservation_id = Some(parse_sandbox_run_arg_value(
                    &words,
                    index,
                    "--reservation-id",
                )?);
            }
            "--agent-id" => {
                if agent_id.is_some() {
                    return Err("stateful sandbox run accepts at most one --agent-id".to_string());
                }
                index += 1;
                agent_id = Some(parse_sandbox_run_arg_value(&words, index, "--agent-id")?);
            }
            "--workspace-id" => {
                if workspace_id.is_some() {
                    return Err(
                        "stateful sandbox run accepts at most one --workspace-id".to_string()
                    );
                }
                index += 1;
                workspace_id = Some(parse_sandbox_run_arg_value(
                    &words,
                    index,
                    "--workspace-id",
                )?);
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
            "--connect-socket" => {
                index += 1;
                connect_sockets.push(parse_sandbox_run_arg_value(
                    &words,
                    index,
                    "--connect-socket",
                )?);
            }
            "--allow-signal" => {
                allow_signal = true;
            }
            "--command" => {
                if inner_command.is_some() {
                    return Err("stateful sandbox run requires exactly one --command".to_string());
                }
                index += 1;
                inner_command = Some(parse_sandbox_run_arg_value(&words, index, "--command")?);
            }
            "--sequence" => {
                index += 1;
                sequences.push(parse_sandbox_run_arg_value(&words, index, "--sequence")?);
            }
            "--sequence-shell" => {
                if sequence_shell.is_some() {
                    return Err(
                        "stateful sandbox run accepts at most one --sequence-shell".to_string()
                    );
                }
                index += 1;
                sequence_shell = Some(parse_sandbox_run_arg_value(
                    &words,
                    index,
                    "--sequence-shell",
                )?);
            }
            "--timeout-seconds" => {
                index += 1;
                let timeout = parse_sandbox_run_arg_value(&words, index, "--timeout-seconds")?;
                timeout_seconds = Some(timeout.parse::<u64>().map_err(|_| {
                    "stateful sandbox run --timeout-seconds requires an integer value".to_string()
                })?);
            }
            "--json" => {}
            "--stream-events" => {
                stream_events = true;
            }
            _ => {
                return Err(format!("unsupported stateful sandbox run argument `{arg}`"));
            }
        }
        index += 1;
    }

    let command = resolve_sandbox_run_command(inner_command, sequences, sequence_shell)?;

    Ok(SandboxRunBashInvocation {
        executable: words[0].clone(),
        request: SandboxRunRequest {
            fs,
            network,
            purpose,
            reservation_id,
            agent_id,
            workspace_id,
            write_targets,
            create_targets,
            write_dirs,
            connect_sockets,
            allow_signal,
            command,
            timeout_seconds,
            stream_events,
        },
    })
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn validate_sequence_shell(shell: &str) -> Result<(), String> {
    if !shell.starts_with('/')
        || shell.contains('\0')
        || shell.contains('\n')
        || shell.contains('\r')
    {
        return Err(
            "stateful sandbox run --sequence-shell requires an absolute shell path".to_string(),
        );
    }
    Ok(())
}

pub(crate) fn resolve_sandbox_run_command(
    command: Option<String>,
    sequences: Vec<String>,
    sequence_shell: Option<String>,
) -> Result<String, String> {
    if command.is_some() && !sequences.is_empty() {
        return Err(
            "stateful sandbox run accepts either --command or --sequence, not both".to_string(),
        );
    }
    if sequences.is_empty() {
        if sequence_shell.is_some() {
            return Err("stateful sandbox run --sequence-shell requires --sequence".to_string());
        }
        let Some(command) = command else {
            return Err(
                "stateful sandbox run requires exactly one --command or at least one --sequence"
                    .to_string(),
            );
        };
        if command.trim().is_empty() {
            return Err("stateful sandbox run requires a non-empty --command".to_string());
        }
        return Ok(command);
    }

    for step in &sequences {
        if step.trim().is_empty() {
            return Err("stateful sandbox run --sequence requires a non-empty value".to_string());
        }
    }

    let shell = sequence_shell.unwrap_or_else(|| "/bin/sh".to_string());
    validate_sequence_shell(&shell)?;

    let mut script = String::from("set -e\n");
    for step in sequences {
        script.push_str(&step);
        script.push('\n');
    }

    Ok(format!("{} -c {}", shell_quote(&shell), shell_quote(&script)))
}

pub(crate) fn parse_sandbox_process_find_bash_invocation(
    command: &str,
) -> Result<SandboxProcessFindBashInvocation, String> {
    reject_outer_shell_syntax(
        command,
        "Bash wrapper must be a single stateful sandbox process find command",
    )?;
    let words = split_simple_command_words(command)?;
    if words.is_empty() {
        return Err("Bash commands must use stateful sandbox process find".to_string());
    }
    if first_word_is_env_assignment(&words[0]) {
        return Err("Bash wrapper must not use outer environment assignments".to_string());
    }
    if words.len() < 4 || words[1] != "sandbox" || words[2] != "process" || words[3] != "find" {
        return Err("Bash commands must use stateful sandbox process find".to_string());
    }

    let mut request = SandboxProcessFindRequest {
        names: Vec::new(),
        contains: Vec::new(),
        pids: Vec::new(),
        parent_pids: Vec::new(),
        process_groups: Vec::new(),
        fields: Vec::new(),
    };
    let mut index = 4;
    while index < words.len() {
        let arg = &words[index];
        match arg.as_str() {
            "--" => {
                return Err("stateful sandbox process find does not support argv mode".to_string());
            }
            "--name" => {
                index += 1;
                request
                    .names
                    .push(parse_sandbox_run_arg_value(&words, index, "--name")?);
            }
            "--contains" => {
                index += 1;
                request
                    .contains
                    .push(parse_sandbox_run_arg_value(&words, index, "--contains")?);
            }
            "--pid" => {
                index += 1;
                request
                    .pids
                    .push(parse_process_selector_arg(&words, index, "--pid")?);
            }
            "--parent-pid" | "--ppid" => {
                index += 1;
                request
                    .parent_pids
                    .push(parse_process_selector_arg(&words, index, arg)?);
            }
            "--process-group" | "--pgid" => {
                index += 1;
                request
                    .process_groups
                    .push(parse_process_selector_arg(&words, index, arg)?);
            }
            "--field" => {
                index += 1;
                request
                    .fields
                    .push(parse_sandbox_run_arg_value(&words, index, "--field")?);
            }
            "--json" => {}
            _ => {
                return Err(format!(
                    "unsupported stateful sandbox process find argument `{arg}`"
                ));
            }
        }
        index += 1;
    }

    Ok(SandboxProcessFindBashInvocation {
        executable: words[0].clone(),
        request,
    })
}

fn parse_process_selector_arg(words: &[String], index: usize, arg: &str) -> Result<u32, String> {
    let value = parse_sandbox_run_arg_value(words, index, arg)?;
    value
        .parse::<u32>()
        .map_err(|_| format!("stateful sandbox process find argument `{arg}` requires an integer"))
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

fn parse_sandbox_fs_profile(value: &str) -> Result<SandboxFsProfile, String> {
    match value {
        "read-only" => Ok(SandboxFsProfile::ReadOnly),
        "write-targets" => Ok(SandboxFsProfile::WriteTargets),
        "external" => Ok(SandboxFsProfile::External),
        "build" => Ok(SandboxFsProfile::Build),
        "git" => Ok(SandboxFsProfile::Git),
        "github-pr" => Ok(SandboxFsProfile::GithubPr),
        _ => Err(
            "stateful sandbox run supports only read-only, write-targets, external, build, git, and github-pr profiles"
                .to_string(),
        ),
    }
}

fn parse_sandbox_network_policy(value: &str) -> Result<SandboxNetworkPolicy, String> {
    match value {
        "disabled" => Ok(SandboxNetworkPolicy::Disabled),
        "enabled" => Ok(SandboxNetworkPolicy::Enabled),
        _ => Err("stateful sandbox run network must be disabled or enabled".to_string()),
    }
}
