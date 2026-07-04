use std::{fs, path::Path};

#[test]
fn grouped_release_pr_title_keeps_release_please_parseable() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("stateful-cli is under crates/");
    let config = fs::read_to_string(repo_root.join("release-please-config.json"))
        .expect("release-please config should be readable");
    let config: serde_json::Value =
        serde_json::from_str(&config).expect("release-please config should be json");

    let pattern = config
        .get("group-pull-request-title-pattern")
        .and_then(serde_json::Value::as_str)
        .expect("grouped release PRs need an explicit title pattern");

    for placeholder in ["${scope}", "${component}", "${version}"] {
        assert!(
            pattern.contains(placeholder),
            "release-please must be able to parse {placeholder} back out of merged release PR titles"
        );
    }
}
