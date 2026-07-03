pub(crate) fn render(template: &str, values: &[(&str, String)]) -> String {
    let mut rendered = template.to_string();
    for (name, value) in values {
        rendered = rendered.replace(&format!("{{{name}}}"), &quote(value));
    }
    rendered
}

pub(crate) fn quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for character in value.chars() {
        if character == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(character);
        }
    }
    quoted.push('\'');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_quotes_shell_metacharacters_as_one_word() {
        assert_eq!(
            render(
                "test -d {workspace}",
                &[("workspace", "a; touch bad #".to_string())],
            ),
            "test -d 'a; touch bad #'"
        );
    }

    #[test]
    fn render_keeps_path_prefix_templates_working() {
        assert_eq!(
            render(
                "cat {workspace}/{agent_id}.txt",
                &[
                    ("workspace", "dir with space".to_string()),
                    ("agent_id", "agent-a".to_string()),
                ],
            ),
            "cat 'dir with space'/'agent-a'.txt"
        );
    }

    #[test]
    fn quote_preserves_single_quotes_as_data() {
        assert_eq!(quote("it's data"), "'it'\\''s data'");
    }
}
