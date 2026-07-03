#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuoteState {
    None,
    Single,
    Double,
}

pub(crate) fn reject_outer_shell_syntax(
    command: &str,
    single_command_message: &str,
) -> Result<(), String> {
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
                '\\' if chars.peek().is_some_and(|next| *next == '\'') => {
                    chars.next();
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

pub(crate) fn split_simple_command_words(command: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut state = QuoteState::None;
    let mut in_word = false;

    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
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
                '\\' if chars.peek().is_some_and(|next| *next == '\'') => {
                    chars.next();
                    current.push('\'');
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

pub(crate) fn first_word_is_env_assignment(word: &str) -> bool {
    let Some((name, _value)) = word.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
        && !name.chars().next().is_some_and(|c| c.is_ascii_digit())
}
