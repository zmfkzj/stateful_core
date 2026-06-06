#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashKind {
    ReadOnly,
    Mutating,
    ValidationBypass,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BashClassification {
    pub kind: BashKind,
    pub reason: String,
}

pub fn classify_bash(command: &str) -> BashClassification {
    let command = command.trim();
    if command.is_empty() {
        return unknown();
    }

    if has_mutating_shell_syntax(command) {
        return mutating("Shell write syntax requires stateful write tools");
    }

    if has_escaped_quote_shell_separator(command) {
        return unknown();
    }

    if has_mutating_find_primary(command) {
        return mutating("Command requires stateful write tools");
    }

    if has_mutating_fd_exec(command) {
        return mutating("Command requires stateful write tools");
    }

    if has_unsupported_shell_syntax(command) {
        return unknown();
    }

    let mut saw_segment = false;
    let mut saw_validation = false;
    let mut saw_unknown = false;
    for segment in command_segments(command) {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        saw_segment = true;

        match classify_segment(segment) {
            BashKind::ReadOnly => {}
            BashKind::Mutating => {
                return mutating("Command requires stateful write tools");
            }
            BashKind::ValidationBypass => {
                saw_validation = true;
            }
            BashKind::Unknown => {
                saw_unknown = true;
            }
        }
    }

    if saw_unknown {
        unknown()
    } else if saw_validation {
        BashClassification {
            kind: BashKind::ValidationBypass,
            reason: "Validation commands must run through validation profiles".to_string(),
        }
    } else if saw_segment {
        BashClassification {
            kind: BashKind::ReadOnly,
            reason: "Known read-only inspection command".to_string(),
        }
    } else {
        unknown()
    }
}

fn classify_segment(segment: &str) -> BashKind {
    if has_unsafe_env_assignment(segment) {
        return BashKind::Unknown;
    }

    let Some(command) = command_name(segment) else {
        return BashKind::Unknown;
    };
    let command = command.as_str();
    if command_has_path_separator(segment) {
        return BashKind::Unknown;
    }

    if matches!(
        command,
        "rm" | "mv"
            | "cp"
            | "mkdir"
            | "touch"
            | "chmod"
            | "chown"
            | "ln"
            | "install"
            | "rsync"
            | "dd"
            | "truncate"
            | "tee"
            | "python"
            | "python3"
            | "perl"
            | "ruby"
            | "node"
            | "rustfmt"
            | "prettier"
    ) {
        return BashKind::Mutating;
    }

    if command == "cargo" {
        return classify_cargo(segment);
    }

    if matches!(command, "npm" | "pnpm" | "yarn" | "bun") {
        return classify_package_manager(segment);
    }

    if matches!(command, "make" | "just") {
        return BashKind::ValidationBypass;
    }

    if command == "git" {
        return classify_git(segment);
    }

    if command == "stateful" {
        return classify_stateful(segment);
    }

    if matches!(command, "stateful-bench" | "stateful-cli" | "stateful-core") {
        return BashKind::Mutating;
    }

    if command == "sed" {
        return classify_sed(segment);
    }

    if command == "rg" {
        if has_option(segment, "--pre") || !has_option(segment, "--no-config") {
            return BashKind::Unknown;
        }
    }

    if command == "sort"
        && (has_option(segment, "-o")
            || has_option(segment, "--output")
            || has_long_option_prefix(segment, "--o")
            || has_option(segment, "--compress-program")
            || has_long_option_prefix(segment, "--com"))
    {
        return BashKind::Mutating;
    }

    if command == "find"
        && contains_any_find_primary(
            segment,
            &[
                "-delete", "-exec", "-execdir", "-ok", "-okdir", "-fprint", "-fprint0", "-fprintf",
                "-fls",
            ],
        )
    {
        return BashKind::Mutating;
    }

    if command == "fd"
        && (has_option(segment, "-x")
            || has_option(segment, "-X")
            || has_option(segment, "--exec")
            || has_option(segment, "--exec-batch"))
    {
        return BashKind::Mutating;
    }

    if command == "yq"
        && (has_option(segment, "-i")
            || has_option(segment, "-s")
            || has_option(segment, "--in-place")
            || has_option(segment, "--inplace")
            || has_option(segment, "--split-exp")
            || has_option(segment, "--split-exp-file"))
    {
        return BashKind::Mutating;
    }

    if command == "xxd"
        && (has_option(segment, "-r")
            || has_positional_output_operand(segment, &["-l", "-c", "-g", "-s", "-o"]))
    {
        return BashKind::Mutating;
    }

    if command == "uniq"
        && has_positional_output_operand(
            segment,
            &[
                "-f",
                "-s",
                "-w",
                "--skip-fields",
                "--skip-chars",
                "--check-chars",
            ],
        )
    {
        return BashKind::Mutating;
    }

    if command == "file"
        && (has_option(segment, "-C")
            || has_option(segment, "--compile")
            || has_long_option_prefix(segment, "--co"))
    {
        return BashKind::Mutating;
    }

    if matches!(
        command,
        "rg" | "grep"
            | "egrep"
            | "fgrep"
            | "cat"
            | "head"
            | "tail"
            | "wc"
            | "nl"
            | "ls"
            | "find"
            | "fd"
            | "pwd"
            | "date"
            | "uname"
            | "whoami"
            | "id"
            | "printf"
            | "echo"
            | "od"
            | "xxd"
            | "hexdump"
            | "stat"
            | "file"
            | "test"
            | "true"
            | "false"
            | "sort"
            | "uniq"
            | "cut"
            | "tr"
            | "jq"
            | "yq"
            | "basename"
            | "dirname"
            | "which"
            | "type"
    ) {
        return BashKind::ReadOnly;
    }

    BashKind::Unknown
}

fn classify_git(segment: &str) -> BashKind {
    let Some(subcommand) = git_subcommand(segment) else {
        return BashKind::Unknown;
    };
    if has_option(segment, "--output") || has_option(segment, "--ext-diff") {
        return BashKind::Mutating;
    }
    match subcommand.as_str() {
        "diff" | "show" | "log" | "blame" => {
            if has_long_option_prefix(segment, "--textc") {
                BashKind::Unknown
            } else if has_option(segment, "--no-ext-diff") && has_option(segment, "--no-textconv") {
                BashKind::ReadOnly
            } else {
                BashKind::Unknown
            }
        }
        "rev-parse" | "ls-files" | "describe" | "merge-base" => BashKind::ReadOnly,
        "grep" => {
            if git_grep_uses_external_command(segment) {
                BashKind::Unknown
            } else {
                BashKind::ReadOnly
            }
        }
        "remote" => classify_git_remote(segment),
        "checkout" | "switch" | "restore" | "reset" | "clean" | "apply" | "merge" | "rebase"
        | "commit" | "add" | "rm" | "mv" | "stash" | "pull" | "push" | "fetch" => {
            BashKind::Mutating
        }
        _ => BashKind::Unknown,
    }
}

fn git_subcommand(segment: &str) -> Option<String> {
    let words = command_words(segment);
    let index = git_subcommand_index(&words)?;
    words.get(index).cloned()
}

fn git_subcommand_index(words: &[String]) -> Option<usize> {
    let mut index = 1;
    while index < words.len() {
        match words[index].as_str() {
            "--no-pager" => index += 1,
            "-C" => index += 2,
            "-c" | "--config-env" => return None,
            word if word.starts_with("--git-dir") || word.starts_with("--work-tree") => {
                return None;
            }
            word if word.starts_with('-') => return None,
            _ => return Some(index),
        }
    }
    None
}

fn classify_git_remote(segment: &str) -> BashKind {
    let words = command_words(segment);
    let Some(remote_index) = git_subcommand_index(&words) else {
        return BashKind::Unknown;
    };
    let mut index = remote_index + 1;
    let mut saw_verbose = false;
    while matches!(
        words.get(index).map(String::as_str),
        Some("-v" | "--verbose")
    ) {
        saw_verbose = true;
        index += 1;
    }
    let Some(subcommand) = words.get(index) else {
        return BashKind::ReadOnly;
    };
    if saw_verbose {
        return BashKind::Unknown;
    }
    match subcommand.as_str() {
        "get-url" => BashKind::ReadOnly,
        "-v" | "--verbose" => BashKind::ReadOnly,
        "add" | "set-url" | "remove" | "rm" | "rename" | "prune" | "update" => BashKind::Mutating,
        _ => BashKind::Unknown,
    }
}

fn git_grep_uses_external_command(segment: &str) -> bool {
    let words = command_words(segment);
    let Some(grep_index) = git_subcommand_index(&words) else {
        return false;
    };

    let mut index = grep_index + 1;
    let mut skip_next = false;
    while index < words.len() {
        let word = &words[index];
        if skip_next {
            skip_next = false;
            index += 1;
            continue;
        }
        if word == "--" {
            break;
        }
        if word.starts_with("--open") || word.starts_with("--textc") {
            return true;
        }
        if matches!(word.as_str(), "--regexp" | "--file" | "-e" | "-f") {
            skip_next = true;
            index += 1;
            continue;
        }
        if word.starts_with("--regexp=") || word.starts_with("--file=") {
            index += 1;
            continue;
        }
        if short_option_cluster_has_option_before_argument(word, 'O', &['e', 'f']) {
            return true;
        }
        if short_option_cluster_consumes_next_argument(word, &['e', 'f']) {
            skip_next = true;
        }
        index += 1;
    }
    false
}

fn short_option_cluster_has_option_before_argument(
    word: &str,
    option: char,
    argument_options: &[char],
) -> bool {
    if !word.starts_with('-') || word.starts_with("--") {
        return false;
    }
    for ch in word[1..].chars() {
        if ch == option {
            return true;
        }
        if argument_options.contains(&ch) {
            return false;
        }
    }
    false
}

fn short_option_cluster_consumes_next_argument(word: &str, argument_options: &[char]) -> bool {
    if !word.starts_with('-') || word.starts_with("--") {
        return false;
    }
    let mut chars = word[1..].chars().peekable();
    while let Some(ch) = chars.next() {
        if argument_options.contains(&ch) {
            return chars.peek().is_none();
        }
    }
    false
}

fn classify_cargo(segment: &str) -> BashKind {
    let Some(subcommand) = nth_word(segment, 1) else {
        return BashKind::Unknown;
    };
    match subcommand.as_str() {
        "test" | "build" | "check" | "clippy" => BashKind::ValidationBypass,
        "fmt" if contains_any_token(segment, &["--check"]) => BashKind::ValidationBypass,
        "fmt" | "run" | "install" | "update" | "add" | "remove" => BashKind::Mutating,
        _ => BashKind::Unknown,
    }
}

fn classify_sed(segment: &str) -> BashKind {
    if has_option(segment, "-i")
        || has_option(segment, "--in-place")
        || has_option(segment, "--inplace")
        || has_option(segment, "-f")
        || has_option(segment, "--file")
    {
        return BashKind::Mutating;
    }

    if !has_option(segment, "-n") {
        return BashKind::Unknown;
    }

    let scripts = sed_scripts(segment);
    if scripts.is_empty() {
        return BashKind::Unknown;
    }

    if scripts
        .iter()
        .all(|script| is_safe_sed_print_script(script))
    {
        BashKind::ReadOnly
    } else {
        BashKind::Unknown
    }
}

fn classify_package_manager(segment: &str) -> BashKind {
    let Some(subcommand) = nth_word(segment, 1) else {
        return BashKind::Mutating;
    };
    match subcommand.as_str() {
        "test" | "run" => BashKind::ValidationBypass,
        _ => BashKind::Mutating,
    }
}

fn classify_stateful(segment: &str) -> BashKind {
    let Some(subcommand) = nth_word(segment, 1) else {
        return BashKind::Unknown;
    };
    match subcommand.as_str() {
        "doctor" | "status" => BashKind::ReadOnly,
        "validate" => BashKind::ValidationBypass,
        "intent" | "commit" | "push" | "sync-outbox" | "hook" | "mcp" | "init" | "install"
        | "server" => BashKind::Mutating,
        _ => BashKind::Unknown,
    }
}

fn mutating(reason: &str) -> BashClassification {
    BashClassification {
        kind: BashKind::Mutating,
        reason: reason.to_string(),
    }
}

fn unknown() -> BashClassification {
    BashClassification {
        kind: BashKind::Unknown,
        reason: "Unrecognized Bash command may require write permission".to_string(),
    }
}

fn command_segments(command: &str) -> Vec<String> {
    split_unquoted(command, &["&&", "||", "|", ";"])
}

fn split_unquoted(input: &str, separators: &[&str]) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut chars = input.char_indices().peekable();
    let mut quote = QuoteState::None;
    let mut escaped = false;

    while let Some((index, ch)) = chars.next() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != QuoteState::Single {
            current.push(ch);
            escaped = true;
            continue;
        }
        update_quote_state(ch, &mut quote);
        if quote == QuoteState::None {
            if let Some(separator) = separators
                .iter()
                .find(|separator| input[index..].starts_with(*separator))
            {
                parts.push(current);
                current = String::new();
                for _ in 1..(*separator).chars().count() {
                    let _ = chars.next();
                }
                continue;
            }
        }
        current.push(ch);
    }

    parts.push(current);
    parts
}

fn has_mutating_shell_syntax(command: &str) -> bool {
    let mut quote = QuoteState::None;
    let mut escaped = false;

    for (index, ch) in command.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != QuoteState::Single {
            escaped = true;
            continue;
        }
        update_quote_state(ch, &mut quote);
        if quote != QuoteState::None {
            continue;
        }

        if ch == '<' && command[index..].starts_with("<<") {
            return true;
        }

        if ch == '>' {
            if is_stderr_dev_null_redirect(command, index) {
                continue;
            }
            return true;
        }
    }

    false
}

fn has_unsupported_shell_syntax(command: &str) -> bool {
    let mut chars = command.char_indices().peekable();
    let mut quote = QuoteState::None;
    let mut escaped = false;

    while let Some((index, ch)) = chars.next() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != QuoteState::Single {
            escaped = true;
            continue;
        }
        update_quote_state(ch, &mut quote);

        if ch == '\n' || ch == '\r' {
            return true;
        }

        if quote == QuoteState::Single {
            continue;
        }

        if ch == '$' {
            return true;
        }

        if ch == '`' {
            return true;
        }

        if quote == QuoteState::None {
            if matches!(ch, '*' | '?' | '[') {
                return true;
            }

            if ch == '{' || ch == '}' {
                return true;
            }

            if ch == '<' && command[index..].starts_with("<(") {
                return true;
            }

            if ch == '&' {
                if command[index..].starts_with("&&") {
                    let _ = chars.next();
                    continue;
                }
                return true;
            }
        }
    }

    false
}

fn is_stderr_dev_null_redirect(command: &str, redirect_index: usize) -> bool {
    let before = command[..redirect_index].trim_end();
    if !before.ends_with('2') {
        return false;
    }
    let after = command[redirect_index + 1..].trim_start();
    let Some(rest) = after.strip_prefix("/dev/null") else {
        return false;
    };
    rest.is_empty()
        || rest
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_whitespace() || matches!(ch, '|' | '&' | ';'))
}

fn command_name(segment: &str) -> Option<String> {
    let words = command_words(segment);
    let word = words.first()?;
    Some(
        word.rsplit('/')
            .next()
            .unwrap_or(word.as_str())
            .trim_start_matches("./")
            .to_string(),
    )
}

fn nth_word(segment: &str, index: usize) -> Option<String> {
    command_words(segment).get(index).cloned()
}

fn command_words(segment: &str) -> Vec<String> {
    let words = words(segment);
    let first_command = words
        .iter()
        .position(|word| !is_env_assignment(word))
        .unwrap_or(words.len());
    words.into_iter().skip(first_command).collect()
}

fn has_option(segment: &str, option: &str) -> bool {
    words(segment)
        .iter()
        .take_while(|word| *word != "--")
        .any(|word| {
            word.as_str() == option
                || (option.starts_with("--")
                    && word
                        .strip_prefix(option)
                        .is_some_and(|suffix| suffix.starts_with('=')))
                || option
                    .strip_prefix('-')
                    .filter(|short| short.len() == 1)
                    .and_then(|short| short.chars().next())
                    .is_some_and(|short| word_has_short_option(word, short))
        })
}

fn word_has_short_option(word: &str, option: char) -> bool {
    if !word.starts_with('-') || word.starts_with("--") {
        return false;
    }
    word[1..].chars().any(|ch| ch == option)
}

fn has_positional_output_operand(segment: &str, options_with_values: &[&str]) -> bool {
    let mut operands = 0;
    let mut after_double_dash = false;
    let mut skip_next = false;
    for word in words(segment).into_iter().skip(1) {
        if skip_next {
            skip_next = false;
        } else if after_double_dash {
            operands += 1;
        } else if word == "--" {
            after_double_dash = true;
        } else if option_consumes_separate_value(&word, options_with_values) {
            skip_next = true;
        } else if word == "-" || !word.starts_with('-') {
            operands += 1;
        }
        if operands > 1 {
            return true;
        }
    }
    false
}

fn option_consumes_separate_value(word: &str, options_with_values: &[&str]) -> bool {
    options_with_values.iter().any(|option| word == *option)
}

fn has_long_option_prefix(segment: &str, prefix: &str) -> bool {
    words(segment)
        .iter()
        .take_while(|word| *word != "--")
        .any(|word| word.starts_with(prefix))
}

fn contains_any_token(segment: &str, tokens: &[&str]) -> bool {
    words(segment)
        .iter()
        .take_while(|word| *word != "--")
        .any(|word| tokens.contains(&word.as_str()))
}

fn contains_any_find_primary(segment: &str, tokens: &[&str]) -> bool {
    words(segment)
        .iter()
        .any(|word| tokens.contains(&word.as_str()))
}

fn has_mutating_find_primary(command: &str) -> bool {
    command_segments(command).iter().any(|segment| {
        command_name(segment).as_deref() == Some("find")
            && contains_any_find_primary(
                segment,
                &[
                    "-delete", "-exec", "-execdir", "-ok", "-okdir", "-fprint", "-fprint0",
                    "-fprintf", "-fls",
                ],
            )
    })
}

fn has_mutating_fd_exec(command: &str) -> bool {
    command_segments(command).iter().any(|segment| {
        command_name(segment).as_deref() == Some("fd")
            && (has_option(segment, "-x")
                || has_option(segment, "-X")
                || has_option(segment, "--exec")
                || has_option(segment, "--exec-batch"))
    })
}

fn has_escaped_quote_shell_separator(command: &str) -> bool {
    let mut chars = command.char_indices().peekable();
    let mut quote = QuoteState::None;
    let mut escaped = false;
    let mut saw_escaped_quote = false;

    while let Some((index, ch)) = chars.next() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != QuoteState::Single {
            if chars
                .peek()
                .map(|(_, next)| matches!(next, '"' | '\''))
                .unwrap_or(false)
            {
                saw_escaped_quote = true;
            }
            escaped = true;
            continue;
        }
        update_quote_state(ch, &mut quote);
        if saw_escaped_quote
            && quote == QuoteState::None
            && (ch == ';' || ch == '|' || (ch == '&' && !command[index..].starts_with("&&")))
        {
            return true;
        }
    }

    false
}

fn has_unsafe_env_assignment(segment: &str) -> bool {
    words(segment)
        .into_iter()
        .take_while(|word| is_env_assignment(word))
        .filter_map(|word| word.split_once("=").map(|(name, _value)| name.to_string()))
        .any(|name| {
            !matches!(
                name.as_str(),
                "LC_ALL" | "LANG" | "TZ" | "NO_COLOR" | "CARGO_TARGET_DIR"
            )
        })
}

fn command_has_path_separator(segment: &str) -> bool {
    command_words(segment)
        .first()
        .is_some_and(|command| command.contains('/'))
}

fn sed_scripts(segment: &str) -> Vec<String> {
    let words = command_words(segment);
    let mut scripts = Vec::new();
    let mut index = 1;
    while index < words.len() {
        let word = &words[index];
        if matches!(word.as_str(), "-n" | "-E" | "-r") {
            index += 1;
            continue;
        }
        if matches!(word.as_str(), "-e" | "--expression") {
            if let Some(script) = words.get(index + 1) {
                scripts.push(script.clone());
            }
            index += 2;
            continue;
        }
        if let Some(script) = word.strip_prefix("-e").filter(|script| !script.is_empty()) {
            scripts.push(script.to_string());
            index += 1;
            continue;
        }
        if word.starts_with('-') {
            index += 1;
            continue;
        }

        if scripts.is_empty() {
            scripts.push(word.clone());
        }
        break;
    }

    scripts
}

fn is_safe_sed_print_script(script: &str) -> bool {
    !script.is_empty()
        && script
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, ',' | '$' | '!' | ';' | 'p' | 'd' | 'q'))
}

fn is_env_assignment(word: &str) -> bool {
    let Some((name, _value)) = word.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = QuoteState::None;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        update_quote_state(ch, &mut quote);
        if quote != QuoteState::Single && ch == '\\' {
            if let Some(next) = chars.next() {
                current.push(next);
            }
            continue;
        }
        if quote == QuoteState::None && ch.is_ascii_whitespace() {
            if !current.is_empty() {
                words.push(current);
                current = String::new();
            }
            continue;
        }
        if matches!(ch, '\'' | '"') {
            continue;
        }
        current.push(ch);
    }

    if !current.is_empty() {
        words.push(current);
    }

    words
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuoteState {
    None,
    Single,
    Double,
}

fn update_quote_state(ch: char, quote: &mut QuoteState) {
    match (*quote, ch) {
        (QuoteState::None, '\'') => *quote = QuoteState::Single,
        (QuoteState::Single, '\'') => *quote = QuoteState::None,
        (QuoteState::None, '"') => *quote = QuoteState::Double,
        (QuoteState::Double, '"') => *quote = QuoteState::None,
        _ => {}
    }
}
