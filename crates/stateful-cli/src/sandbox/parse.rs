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

    Ok(format!(
        "{} -c {}",
        shell_quote(&shell),
        shell_quote(&script)
    ))
}
