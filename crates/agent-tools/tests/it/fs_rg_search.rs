//! `srv:fs/rg_search` 的功能与确定性资源预算。

use agent_tools::ToolExecutor;
use serde_json::{Value, json};
use crate::support::TestRoot;

fn run_search(root: &TestRoot, input: Value) -> Value {
    serde_json::from_str(&run_raw(root, input)).unwrap()
}

fn run_raw(root: &TestRoot, input: Value) -> String {
    ToolExecutor::new(root.path())
        .unwrap()
        .execute("srv:fs/rg_search", &input)
        .unwrap()
}

#[test]
fn returns_relative_path_line_column_and_a_capped_line() {
    let root = TestRoot::new("rg-search-happy");
    root.write("a.txt", "no match\nneedle first\n");
    root.write("dir/b.txt", "xx needle second\n");

    let result = run_search(&root, json!({ "query": "needle", "max_line_chars": 10 }));
    assert_eq!(
        result,
        json!({
            "matches": [
                {
                    "path": "a.txt",
                    "line": 2,
                    "column": 1,
                    "text": "needle fir…",
                    "line_truncated": true,
                },
                {
                    "path": "dir/b.txt",
                    "line": 1,
                    "column": 4,
                    "text": "xx needle …",
                    "line_truncated": true,
                },
            ],
            "truncated": false,
        })
    );
}

#[test]
fn preserves_whitespace_in_the_literal_needle() {
    let root = TestRoot::new("rg-search-literal-whitespace");
    root.write("a.txt", "a needle b\nneedle\n");

    let result = run_search(&root, json!({ "query": " needle " }));
    let matches = result["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["line"], json!(1));
    assert_eq!(matches[0]["column"], json!(2));
}

#[test]
fn max_results_and_line_length_bound_the_result_without_timing_assertions() {
    let root = TestRoot::new("rg-search-budget");
    for index in 0..128 {
        root.write(
            &format!("src/file-{index:03}.txt"),
            "needle: xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n",
        );
    }

    let first = run_search(
        &root,
        json!({ "query": "needle", "max_results": 16, "max_line_chars": 12 }),
    );
    let second = run_search(
        &root,
        json!({ "query": "needle", "max_results": 16, "max_line_chars": 12 }),
    );
    assert_eq!(first, second, "同一输入的搜索结果必须稳定");
    let matches = first["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 16);
    assert_eq!(matches[0]["path"], json!("src/file-000.txt"));
    assert!(
        matches
            .iter()
            .all(|hit| hit["text"].as_str().unwrap().chars().count() <= 13)
    );
    assert_eq!(first["truncated"], json!(true));
    assert!(serde_json::to_string(&first).unwrap().len() < 2_500);
}

#[test]
fn exact_result_limit_is_complete_when_no_additional_match_exists() {
    let root = TestRoot::new("rg-search-exact-limit");
    root.write("a.txt", "needle one\nneedle two\nneedle three\n");

    let result = run_search(&root, json!({ "query": "needle", "max_results": 3 }));
    assert_eq!(result["matches"].as_array().unwrap().len(), 3);
    assert_eq!(result["truncated"], json!(false));
}

#[test]
fn maximum_user_limits_still_produce_complete_json_within_byte_budget() {
    let root = TestRoot::new("rg-search-json-budget");
    for index in 0..32 {
        root.write(
            &format!("src/file-{index:03}.txt"),
            &format!("needle{}\n", "x".repeat(4_090)),
        );
    }

    let input = json!({
        "query": "needle",
        "max_results": 1000,
        "max_line_chars": 4096,
    });
    let first = run_raw(&root, input.clone());
    let second = run_raw(&root, input);
    assert_eq!(first, second, "响应预算也必须保持确定性");
    assert!(first.len() <= 24 * 1024);

    let result: Value = serde_json::from_str(&first).unwrap();
    let matches = result["matches"].as_array().unwrap();
    assert!(!matches.is_empty());
    assert!(matches.len() < 32);
    assert_eq!(result["truncated"], json!(true));
}

#[test]
fn skips_an_over_budget_file_and_reports_an_incomplete_search() {
    let root = TestRoot::new("rg-search-file-budget");
    root.write("large.txt", &format!("needle{}", "x".repeat(1_048_576)));

    let result = run_search(&root, json!({ "query": "needle" }));
    assert_eq!(result, json!({ "matches": [], "truncated": true }));
}

#[test]
fn rejects_unknown_arguments_and_paths_outside_root() {
    let root = TestRoot::new("rg-search-invalid");
    let executor = ToolExecutor::new(root.path()).unwrap();

    let unknown = executor
        .execute("srv:fs/rg_search", &json!({ "query": "a", "regex": true }))
        .unwrap_err();
    assert_eq!(&*unknown.code, "bad_input");

    let outside = executor
        .execute("srv:fs/rg_search", &json!({ "query": "a", "path": "../" }))
        .unwrap_err();
    assert_eq!(&*outside.code, "outside_root");
}
