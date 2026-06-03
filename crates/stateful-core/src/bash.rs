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
    let trimmed = command.trim();
    let normalized = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");

    if contains_write_syntax(trimmed) {
        return BashClassification {
            kind: BashKind::Mutating,
            reason: "command uses shell redirection or mutation syntax".to_string(),
        };
    }

    if contains_shell_control_syntax(trimmed) {
        return BashClassification {
            kind: BashKind::Mutating,
            reason: "command uses shell control syntax".to_string(),
        };
    }

    if contains_unsupported_shell_expansion(trimmed) {
        return BashClassification {
            kind: BashKind::Mutating,
            reason: "command uses unsupported shell expansion syntax".to_string(),
        };
    }

    if parse_shell_words(trimmed).is_err() {
        return BashClassification {
            kind: BashKind::Mutating,
            reason: "command uses unsupported shell quoting".to_string(),
        };
    }

    if is_validation_command(&normalized) {
        return BashClassification {
            kind: BashKind::ReadOnly,
            reason: "test and validation commands do not directly edit code".to_string(),
        };
    }

    if is_codex_read_only_command(&normalized) {
        return BashClassification {
            kind: BashKind::ReadOnly,
            reason: "codex status commands are read-only benchmark preflight checks".to_string(),
        };
    }

    if is_stateful_escape_hatch(&normalized) {
        return BashClassification {
            kind: BashKind::ReadOnly,
            reason: "command matches stateful diagnostic or controlled validation allowlist"
                .to_string(),
        };
    }

    if is_stateful_intent_declare_command(trimmed) {
        return BashClassification {
            kind: BashKind::ReadOnly,
            reason: "stateful intent declaration is a coordination gate, not a code write"
                .to_string(),
        };
    }

    if is_stateful_commit_command(trimmed) {
        return BashClassification {
            kind: BashKind::ReadOnly,
            reason: "stateful commit is a structured git operation with explicit path checks"
                .to_string(),
        };
    }

    if is_stateful_command(&normalized) {
        return BashClassification {
            kind: BashKind::Mutating,
            reason: "stateful control commands that change coordination state must use MCP tools"
                .to_string(),
        };
    }

    if let Some(classification) = classify_stateful_bench_command(&normalized) {
        return classification;
    }

    if is_read_only_command(&normalized) {
        return BashClassification {
            kind: BashKind::ReadOnly,
            reason: "command matches read/search allowlist".to_string(),
        };
    }

    if is_known_mutating_command(&normalized) {
        return BashClassification {
            kind: BashKind::Mutating,
            reason: "command is known to mutate files or runtime state".to_string(),
        };
    }

    BashClassification {
        kind: BashKind::Unknown,
        reason: "command is outside the read/search allowlist".to_string(),
    }
}

fn contains_write_syntax(command: &str) -> bool {
    command.contains('>')
        || command.contains(" >")
        || command.contains(">>")
        || command.contains(" 2>")
        || command.contains("| tee")
        || command.contains("|tee")
        || command.contains("<<")
}

fn is_validation_command(command: &str) -> bool {
    let words = shell_words(command);
    if let Some(first) = words.first()
        && is_python_binary(first)
        && matches!(
            words.get(1).map(String::as_str),
            Some("tests/runtests.py") | Some("-m")
        )
    {
        return true;
    }

    matches!(
        first_words(command, 2).as_deref(),
        Some("cargo test")
            | Some("npm test")
            | Some("pnpm test")
            | Some("yarn test")
            | Some("pytest")
            | Some("go test")
    ) || command == "cargo test"
        || command.starts_with("cargo test ")
        || command.starts_with("pytest ")
        || command.starts_with("go test ")
}

fn is_codex_read_only_command(command: &str) -> bool {
    let words = shell_words(command);
    matches!(
        words.as_slice(),
        [first, flag] if first == "codex" && matches!(flag.as_str(), "--help" | "-h" | "--version" | "-V")
    ) || matches!(
        words.as_slice(),
        [first, subcommand, flag] if first == "codex"
            && matches!(subcommand.as_str(), "auth" | "login" | "doctor" | "exec")
            && matches!(flag.as_str(), "--help" | "-h")
    ) || matches!(
        words.as_slice(),
        [first, second, third] if first == "codex" && second == "auth" && third == "status"
    ) || matches!(
        words.as_slice(),
        [first, second, third] if first == "codex" && second == "login" && third == "status"
    ) || matches!(
        words.as_slice(),
        [first, second] if first == "codex" && second == "doctor"
    ) || matches!(
        words.as_slice(),
        [first, second, third] if first == "codex" && second == "doctor" && third == "--json"
    )
}

fn is_stateful_escape_hatch(command: &str) -> bool {
    let words = shell_words(command);
    if words.len() < 2 || !is_stateful_binary(&words[0]) {
        return false;
    }

    matches!(
        words[1].as_str(),
        "doctor" | "current" | "events" | "status"
    ) || words[1] == "validate" && words.len() >= 3
}

fn is_stateful_command(command: &str) -> bool {
    shell_words(command)
        .first()
        .is_some_and(|word| is_stateful_binary(word))
}

fn is_stateful_intent_declare_command(command: &str) -> bool {
    if contains_shell_control_syntax(command) {
        return false;
    }
    let words = shell_words(command);
    words.len() >= 3
        && is_stateful_binary(&words[0])
        && words[1] == "intent"
        && words[2] == "declare"
}

fn is_stateful_commit_command(command: &str) -> bool {
    if contains_shell_control_syntax(command) {
        return false;
    }

    let words = shell_words(command);
    let Some(separator) = words.iter().position(|word| word == "--") else {
        return false;
    };
    let paths = &words[separator + 1..];

    words.len() >= 6
        && is_stateful_binary(&words[0])
        && words[1] == "commit"
        && words.iter().any(|word| word == "-m" || word == "--message")
        && !paths.is_empty()
        && paths.iter().all(|path| !is_broad_commit_pathspec(path))
}

fn is_broad_commit_pathspec(path: &str) -> bool {
    path.is_empty()
        || path == "."
        || path == "*"
        || path == ":/"
        || path.starts_with('/')
        || path.starts_with('-')
        || path.starts_with(':')
        || path.starts_with("./")
        || path.contains("..")
        || path.contains("//")
        || path.contains("/./")
        || path.ends_with('/')
        || path.contains('*')
        || path.contains('?')
        || path.contains('[')
        || path.contains(']')
}

fn contains_shell_control_syntax(command: &str) -> bool {
    ["\n", "\r", ";", "&", "&&", "||", "|", "`", "$(", "<(", ">("]
        .iter()
        .any(|token| command.contains(token))
}

fn contains_unsupported_shell_expansion(command: &str) -> bool {
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escaped = false;

    for ch in command.chars() {
        if escaped {
            escaped = false;
            if ch == '$'
                || (!in_single_quote && !in_double_quote && is_shell_expansion_metachar(ch))
            {
                return true;
            }
            continue;
        }

        if ch == '\\' && !in_single_quote {
            escaped = true;
            continue;
        }

        if ch == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            continue;
        }

        if ch == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            continue;
        }

        if ch == '$' || (!in_single_quote && !in_double_quote && is_shell_expansion_metachar(ch)) {
            return true;
        }
    }

    false
}

fn is_shell_expansion_metachar(ch: char) -> bool {
    matches!(ch, '{' | '}' | '*' | '?' | '[' | ']')
}

fn is_stateful_binary(word: &str) -> bool {
    word == "stateful" || word.ends_with("/stateful")
}

fn classify_stateful_bench_command(command: &str) -> Option<BashClassification> {
    let words = shell_words(command);
    let first = words.first()?;
    if !is_stateful_bench_binary(first) {
        return None;
    }

    let Some(subcommand) = words.get(1).map(String::as_str) else {
        return Some(BashClassification {
            kind: BashKind::ReadOnly,
            reason: "stateful-bench help and diagnostics are operational commands".to_string(),
        });
    };

    if !matches!(
        subcommand,
        "fetch"
            | "prepare-pairs"
            | "generate-fallback-preflight"
            | "sample"
            | "run"
            | "report"
            | "compare"
            | "--help"
            | "-h"
    ) {
        return Some(BashClassification {
            kind: BashKind::Unknown,
            reason: "stateful-bench subcommand is not recognized by the bash policy".to_string(),
        });
    }

    for path in stateful_bench_path_arguments(&words) {
        if is_protected_code_path(path) {
            return Some(BashClassification {
                kind: BashKind::Mutating,
                reason: format!(
                    "stateful-bench command targets protected code path `{path}`; use a controlled validation profile or structured edit"
                ),
            });
        }
    }

    Some(BashClassification {
        kind: BashKind::ReadOnly,
        reason: "stateful-bench command is operational and limited to benchmark artifacts"
            .to_string(),
    })
}

fn is_stateful_bench_binary(word: &str) -> bool {
    word == "stateful-bench" || word.ends_with("/stateful-bench")
}

fn stateful_bench_path_arguments(words: &[String]) -> Vec<&str> {
    let path_flags = [
        "--output",
        "--output-dir",
        "--pairs",
        "--dataset",
        "--input",
        "--run-dir",
        "--stateful-run-dir",
        "--no-state-run-dir",
        "--manifest",
        "--fallback-preflight",
    ];
    let mut paths = Vec::new();
    for index in 0..words.len() {
        let word = words[index].as_str();
        if let Some((flag, value)) = word.split_once('=') {
            if path_flags.contains(&flag) {
                paths.push(value);
            }
        } else if path_flags.contains(&word)
            && let Some(value) = words.get(index + 1)
        {
            paths.push(value.as_str());
        }
    }
    paths
}

fn is_protected_code_path(path: &str) -> bool {
    let normalized = normalize_command_path(path);
    if normalized.is_empty() {
        return false;
    }

    if normalized.starts_with(".stateful_bench/")
        || normalized == ".stateful_bench"
        || normalized.starts_with("target/")
        || normalized == "target"
        || normalized.starts_with("/tmp/")
        || normalized.starts_with("private/var/")
        || normalized.starts_with("var/folders/")
    {
        return false;
    }

    normalized == "Cargo.toml"
        || normalized == "Cargo.lock"
        || normalized == ".gitignore"
        || normalized == ".stateful"
        || normalized.starts_with(".stateful/")
        || normalized == ".codex"
        || normalized.starts_with(".codex/")
        || normalized == "crates"
        || normalized.starts_with("crates/")
        || normalized == "docs"
        || normalized.starts_with("docs/")
        || normalized == "src"
        || normalized.starts_with("src/")
        || normalized == "tests"
        || normalized.starts_with("tests/")
}

fn normalize_command_path(path: &str) -> String {
    path.trim_matches(&['"', '\''][..])
        .replace('\\', "/")
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .fold(Vec::new(), |mut segments, segment| {
            if segment == ".." {
                segments.pop();
            } else {
                segments.push(segment);
            }
            segments
        })
        .join("/")
}

fn is_read_only_command(command: &str) -> bool {
    let words = shell_words(command);
    let Some(first) = words.first().map(String::as_str) else {
        return true;
    };

    match first {
        "pwd" | "ls" | "rg" | "cat" | "head" | "tail" | "wc" | "which" => true,
        "find" => !find_has_mutating_action(&words),
        "sed" => command.starts_with("sed -n "),
        "git" => is_read_only_git_command(&words),
        "docker" => matches!(
            first_words(command, 2).as_deref(),
            Some("docker info") | Some("docker version")
        ),
        "colima" => matches!(
            first_words(command, 2).as_deref(),
            Some("colima status") | Some("colima start")
        ),
        _ => false,
    }
}

fn is_read_only_git_command(words: &[String]) -> bool {
    match words.get(1).map(String::as_str) {
        Some("status" | "show" | "log" | "rev-parse") => true,
        Some("diff") => !git_diff_has_output_option(words),
        Some("branch") => git_branch_is_read_only(words),
        _ => false,
    }
}

fn is_python_binary(word: &str) -> bool {
    word == "python" || word == "python3" || word.ends_with("/python") || word.ends_with("/python3")
}

fn is_known_mutating_command(command: &str) -> bool {
    let words = shell_words(command);
    let Some(first) = words.first().map(String::as_str) else {
        return false;
    };

    if matches!(
        first,
        "rm" | "mv" | "cp" | "mkdir" | "touch" | "chmod" | "chown" | "codex"
    ) {
        return true;
    }

    if first == "find" && find_has_mutating_action(&words) {
        return true;
    }

    if first == "git" && git_has_mutating_read_allowlist_option(&words) {
        return true;
    }

    matches!(
        first_words(command, 2).as_deref(),
        Some("git checkout")
            | Some("git switch")
            | Some("git restore")
            | Some("git reset")
            | Some("git clean")
            | Some("git apply")
            | Some("git merge")
            | Some("git rebase")
    )
}

fn find_has_mutating_action(words: &[String]) -> bool {
    words.iter().any(|word| {
        matches!(
            word.as_str(),
            "-delete"
                | "-exec"
                | "-execdir"
                | "-ok"
                | "-okdir"
                | "-fprint"
                | "-fprint0"
                | "-fprintf"
                | "-fls"
        )
    })
}

fn git_has_mutating_read_allowlist_option(words: &[String]) -> bool {
    match words.get(1).map(String::as_str) {
        Some("branch") => !git_branch_is_read_only(words),
        Some("diff") => git_diff_has_output_option(words),
        _ => false,
    }
}

fn git_branch_is_read_only(words: &[String]) -> bool {
    let args = &words[2..];
    if args.is_empty() || matches!(args, [arg] if arg == "--show-current") {
        return true;
    }

    let mut has_list = false;
    for arg in args {
        if arg == "--list" {
            has_list = true;
            continue;
        }
        if is_read_only_git_branch_flag(arg) {
            continue;
        }
        if has_list && !arg.starts_with('-') {
            continue;
        }
        return false;
    }

    true
}

fn is_read_only_git_branch_flag(arg: &str) -> bool {
    matches!(
        arg,
        "--all" | "-a" | "--remotes" | "-r" | "--verbose" | "-v" | "-vv"
    )
}

fn git_diff_has_output_option(words: &[String]) -> bool {
    words
        .iter()
        .skip(2)
        .any(|word| word == "--output" || word.starts_with("--output="))
}

fn first_words(command: &str, count: usize) -> Option<String> {
    let words = shell_words(command);
    if words.len() >= count {
        Some(words.into_iter().take(count).collect::<Vec<_>>().join(" "))
    } else {
        None
    }
}

fn shell_words(command: &str) -> Vec<String> {
    parse_shell_words(command).unwrap_or_default()
}

fn parse_shell_words(command: &str) -> Result<Vec<String>, ShellWordError> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut in_word = false;
    let mut chars = command.chars();

    while let Some(ch) = chars.next() {
        match quote {
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            Some('"') => {
                if ch == '"' {
                    quote = None;
                } else if ch == '\\' {
                    let Some(next) = chars.next() else {
                        return Err(ShellWordError::TrailingEscape);
                    };
                    current.push(next);
                } else {
                    current.push(ch);
                }
            }
            None if ch == '\'' || ch == '"' => {
                in_word = true;
                quote = Some(ch);
            }
            None if ch == '\\' => {
                in_word = true;
                let Some(next) = chars.next() else {
                    return Err(ShellWordError::TrailingEscape);
                };
                current.push(next);
            }
            None if ch.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            None => {
                in_word = true;
                current.push(ch);
            }
            Some(_) => unreachable!(),
        }
    }

    if quote.is_some() {
        return Err(ShellWordError::UnmatchedQuote);
    }
    if in_word {
        words.push(current);
    }

    Ok(words)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellWordError {
    UnmatchedQuote,
    TrailingEscape,
}
