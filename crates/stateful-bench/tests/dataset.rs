use stateful_bench::{SweBenchInstance, fetch_rows_to_writer, parse_rows_page};

#[test]
fn rows_page_parser_normalizes_swe_bench_rows() {
    let page = parse_rows_page(
        r#"{
          "features": [],
          "rows": [
            {
              "row_idx": 0,
              "row": {
                "instance_id": "django__django-12345",
                "repo": "django/django",
                "base_commit": "abc123",
                "problem_statement": "Fix the issue",
                "version": "4.2",
                "patch": "diff --git a/django/core/a.py b/django/core/a.py\n",
                "test_patch": "diff --git a/tests/test_a.py b/tests/test_a.py\n",
                "FAIL_TO_PASS": "[\"tests/test_a.py::test_fix\"]",
                "PASS_TO_PASS": ["tests/test_a.py::test_existing"],
                "difficulty": "medium"
              },
              "truncated_cells": []
            }
          ],
          "num_rows_total": 1,
          "num_rows_per_page": 100,
          "partial": false
        }"#,
    )
    .expect("rows page should parse");

    assert_eq!(page.num_rows_total, 1);
    assert_eq!(page.instances.len(), 1);

    let instance = &page.instances[0];
    assert_eq!(instance.instance_id, "django__django-12345");
    assert_eq!(instance.repo, "django/django");
    assert_eq!(instance.version.as_deref(), Some("4.2"));
    assert_eq!(instance.fail_to_pass, vec!["tests/test_a.py::test_fix"]);
    assert_eq!(
        instance.pass_to_pass,
        vec!["tests/test_a.py::test_existing"]
    );
    assert_eq!(instance.difficulty.as_deref(), Some("medium"));
}

#[test]
fn instance_normalization_accepts_missing_optional_fields() {
    let value = serde_json::json!({
        "instance_id": "sympy__sympy-1",
        "repo": "sympy/sympy",
        "base_commit": "def456",
        "problem_statement": "Fix another issue",
        "patch": "",
        "test_patch": "",
        "FAIL_TO_PASS": [],
        "PASS_TO_PASS": "[]"
    });

    let instance: SweBenchInstance =
        serde_json::from_value(value).expect("instance should deserialize");

    assert_eq!(instance.version, None);
    assert_eq!(instance.fail_to_pass, Vec::<String>::new());
    assert_eq!(instance.pass_to_pass, Vec::<String>::new());
}

#[test]
fn rows_fetcher_paginates_mocked_rows_until_total_is_reached() {
    let mut calls = Vec::new();
    let mut output = Vec::new();

    let fetched = fetch_rows_to_writer(2, &mut output, |offset, length| {
        calls.push((offset, length));
        Ok(mock_rows_page(offset))
    })
    .expect("mock pagination should fetch");

    assert_eq!(fetched, 3);
    assert_eq!(calls, vec![(0, 2), (2, 2)]);

    let lines = String::from_utf8(output)
        .expect("jsonl should be utf8")
        .lines()
        .count();
    assert_eq!(lines, 3);
}

fn mock_rows_page(offset: usize) -> String {
    let rows = match offset {
        0 => vec![mock_row("django__django-1"), mock_row("django__django-2")],
        2 => vec![mock_row("django__django-3")],
        _ => Vec::new(),
    };
    serde_json::json!({
        "features": [],
        "rows": rows,
        "num_rows_total": 3,
        "num_rows_per_page": 2,
        "partial": false
    })
    .to_string()
}

fn mock_row(instance_id: &str) -> serde_json::Value {
    serde_json::json!({
        "row_idx": 0,
        "row": {
            "instance_id": instance_id,
            "repo": "django/django",
            "base_commit": "abc123",
            "problem_statement": "Fix",
            "patch": "",
            "test_patch": "",
            "FAIL_TO_PASS": [],
            "PASS_TO_PASS": []
        },
        "truncated_cells": []
    })
}
