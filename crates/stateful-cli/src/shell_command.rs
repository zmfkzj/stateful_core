#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuoteState {
    None,
    Single,
    Double,
}

pub(crate) fn split_simple_command_words(command: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut state = QuoteState::None;
    let mut in_word = false;

    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
        match state {
            QuoteState::None => match ch {
                '\n' | '\r' | ';' | '|' | '&' | '<' | '>' | '`' | '$' | '*' | '?' | '[' | ']'
                | '{' | '}' | '~' | '(' | ')' | '#' | '!' => {
                    return Err("Bash wrapper command must be a single literal command".to_string());
                }
                '\'' => {
                    state = QuoteState::Single;
                    in_word = true;
                }
                '"' => {
                    state = QuoteState::Double;
                    in_word = true;
                }
                '\\' if chars.peek().is_some_and(|next| *next == '\'') => {
                    chars.next();
                    current.push('\'');
                    in_word = true;
                }
                '\\' => {
                    return Err("Bash wrapper command must not use shell escapes".to_string());
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
                } else if matches!(ch, '`' | '$' | '\\') {
                    return Err("Bash wrapper command must not use shell expansion".to_string());
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

pub(crate) fn first_word_is_env_assignment(word: &str) -> bool {
    let Some((name, _value)) = word.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
        && !name.chars().next().is_some_and(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::split_simple_command_words;

    #[test]
    fn parses_literal_single_quoted_wrapper_arguments() {
        assert_eq!(
            split_simple_command_words("'/tmp/stateful' status --value '{\"path\":\"a b\"}'")
                .expect("literal wrapper should parse"),
            ["/tmp/stateful", "status", "--value", "{\"path\":\"a b\"}"]
        );
    }

    #[test]
    fn rejects_shell_control_and_expansion() {
        for command in [
            "/tmp/stateful status; bypass",
            "/tmp/stateful status $(bypass)",
            "/tmp/stateful status *",
            "/tmp/stateful status {one,two}",
            "~/stateful status",
            r"/tmp/stateful status a\ b",
            "/tmp/stateful status (bypass)",
            "/tmp/stateful status # bypass",
            "/tmp/stateful status !",
            r#""/tmp/stateful" status "a\"b""#,
        ] {
            assert!(
                split_simple_command_words(command).is_err(),
                "`{command}` should be rejected"
            );
        }
    }
}
